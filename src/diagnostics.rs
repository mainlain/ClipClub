//! Bounded checks of the running client, temporary storage, and TCP reachability.
//! TCP probes deliberately send no application data: a reachable socket does
//! not establish a successful ClipClub or BitTorrent protocol handshake.

use crate::peer::PeerDiagnostics;
use anyhow::{Result, ensure};
use futures::{StreamExt, stream};
use libp2p::{Multiaddr, multiaddr::Protocol};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    future::Future,
    io::{self, Read, Seek, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, net::TcpStream, process::Command};

const MAX_REMOTE_PROBES: usize = 16;
const PROBE_CONCURRENCY: usize = 8;
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSuite {
    #[default]
    All,
    Network,
    Device,
    Media,
    Wallet,
}

impl DiagnosticSuite {
    pub fn includes(self, suite: Self) -> bool {
        self == Self::All || self == suite
    }

    fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Network => "network",
            Self::Device => "device",
            Self::Media => "media",
            Self::Wallet => "wallet",
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticRequest {
    #[serde(default)]
    pub suite: DiagnosticSuite,
    pub peer_ip: Option<Ipv4Addr>,
    pub p2p_port: Option<u16>,
    pub torrent_port: Option<u16>,
}

impl DiagnosticRequest {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.p2p_port != Some(0),
            "Peer port must be between 1 and 65535"
        );
        ensure!(
            self.torrent_port != Some(0),
            "Video port must be between 1 and 65535"
        );
        if let Some(ip) = self.peer_ip {
            ensure!(
                !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast(),
                "Enter a unicast IPv4 address for the other device"
            );
        }
        Ok(())
    }
}

pub struct DiagnosticContext {
    pub root: PathBuf,
    pub probe: PathBuf,
    pub peer_id: String,
    pub p2p_port: u16,
    pub torrent_port: u16,
    pub local_seeded_clips: usize,
    pub catalog_clips: usize,
    pub ready_clips: usize,
    pub connected_presence: usize,
    pub peer_video_endpoints: Vec<SocketAddr>,
    pub downloading: Option<(u64, u64)>,
    pub recent_download_failures: usize,
    pub cached_clips: usize,
    pub rating_session_active: bool,
    pub wallet_checks: Vec<DiagnosticCheck>,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticReport {
    pub suite: DiagnosticSuite,
    pub generated_at_ms: u64,
    pub summary: String,
    pub checks: Vec<DiagnosticCheck>,
    pub report: String,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticCheck {
    pub category: &'static str,
    pub name: String,
    pub status: &'static str,
    pub detail: String,
}

impl DiagnosticCheck {
    pub(crate) fn new(
        category: &'static str,
        name: impl Into<String>,
        status: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            category,
            name: name.into(),
            status,
            detail: detail.into(),
        }
    }
}

#[derive(Clone)]
struct Probe {
    label: String,
    address: SocketAddr,
    local: bool,
    video: bool,
}

#[derive(Debug, PartialEq)]
enum Reachability {
    Reachable,
    Refused,
    TimedOut,
    Other(String),
}

async fn connect_result<F>(connect: F, timeout: Duration) -> Reachability
where
    F: Future<Output = io::Result<TcpStream>>,
{
    match tokio::time::timeout(timeout, connect).await {
        Ok(Ok(_stream)) => Reachability::Reachable,
        Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => Reachability::Refused,
        Ok(Err(error)) if error.kind() == io::ErrorKind::TimedOut => Reachability::TimedOut,
        Ok(Err(error)) => Reachability::Other(error.to_string()),
        Err(_) => Reachability::TimedOut,
    }
}

fn tcp_endpoint(address: &str) -> Option<SocketAddr> {
    let address: Multiaddr = address.parse().ok()?;
    let mut parts = address.iter();
    let ip = match parts.next()? {
        Protocol::Ip4(ip) => IpAddr::V4(ip),
        Protocol::Ip6(ip) => IpAddr::V6(ip),
        _ => return None,
    };
    let Protocol::Tcp(port) = parts.next()? else {
        return None;
    };
    if port == 0 || ip.is_unspecified() || ip.is_multicast() {
        return None;
    }
    match parts.next() {
        Some(Protocol::P2p(_)) | None => (),
        _ => return None,
    }
    parts.next().is_none().then_some(SocketAddr::new(ip, port))
}

fn local_network(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
        IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local(),
    }
}

fn probes(
    context: &DiagnosticContext,
    mesh: Option<&PeerDiagnostics>,
    request: &DiagnosticRequest,
) -> Vec<Probe> {
    let mut result = vec![
        Probe {
            label: "Local peer mesh listener".into(),
            address: SocketAddr::from((Ipv4Addr::LOCALHOST, context.p2p_port)),
            local: true,
            video: false,
        },
        Probe {
            label: "Local video listener".into(),
            address: SocketAddr::from((Ipv4Addr::LOCALHOST, context.torrent_port)),
            local: true,
            video: true,
        },
    ];
    let mut remote = Vec::new();
    if let Some(ip) = request.peer_ip {
        remote.push(Probe {
            label: "Other device peer mesh".into(),
            address: SocketAddr::from((ip, request.p2p_port.unwrap_or(context.p2p_port))),
            local: false,
            video: false,
        });
        remote.push(Probe {
            label: "Other device video transfer".into(),
            address: SocketAddr::from((ip, request.torrent_port.unwrap_or(context.torrent_port))),
            local: false,
            video: true,
        });
    }
    let mut video_endpoints = context.peer_video_endpoints.clone();
    video_endpoints.sort();
    video_endpoints.dedup();
    for address in video_endpoints {
        if address.port() != 0 && !address.ip().is_unspecified() && !address.ip().is_multicast() {
            remote.push(Probe {
                label: "Connected peer video transfer".into(),
                address,
                local: false,
                video: true,
            });
        }
    }
    if let Some(mesh) = mesh {
        for address in &mesh.mdns_endpoints {
            if let Some(address) = tcp_endpoint(address)
                && local_network(address.ip())
            {
                remote.push(Probe {
                    label: "Discovered local peer mesh".into(),
                    address,
                    local: false,
                    video: false,
                });
            }
        }
    }
    let mut seen = HashSet::new();
    result.extend(
        remote
            .into_iter()
            .filter(|probe| seen.insert((probe.address, probe.video)))
            .take(MAX_REMOTE_PROBES),
    );
    result
}

fn probe_check(probe: &Probe, result: &Reachability, elapsed: Duration) -> DiagnosticCheck {
    let name = format!("{} · {}", probe.label, probe.address);
    let (status, detail) = match result {
        Reachability::Reachable => (
            "pass",
            format!(
                "TCP {} reachable in {} ms; application protocol and video contents were not checked.{}",
                probe.address,
                elapsed.as_millis(),
                if probe.local {
                    " This checks this device only; it does not establish reachability from another device."
                } else {
                    " This checks this device's outbound path only."
                }
            ),
        ),
        Reachability::Refused => (
            "fail",
            "TCP connection refused. The target may have no listener on this port, or a firewall may reject it.".into(),
        ),
        Reachability::TimedOut => (
            "fail",
            "TCP connection timed out after three seconds. A firewall, network filtering, routing, or an unavailable device may be responsible; this test cannot identify which.".into(),
        ),
        Reachability::Other(error) => ("fail", format!("TCP connection failed: {error}")),
    };
    DiagnosticCheck::new("network", name, status, detail)
}

/// A blocking filesystem call may finish after the caller's deadline. Keeping
/// ownership in this worker ensures a late open still reaches cleanup.
struct TemporaryDiagnosticFile {
    path: Option<PathBuf>,
    file: Option<std::fs::File>,
}

impl Drop for TemporaryDiagnosticFile {
    fn drop(&mut self) {
        drop(self.file.take());
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

async fn storage_round_trip(directory: &Path) -> io::Result<()> {
    let directory = directory.to_owned();
    let worker = tokio::task::spawn_blocking(move || {
        let path = directory.join(format!(
            ".clipclub-diagnostic-{:032x}",
            rand::random::<u128>()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // A failed create_new never gives us ownership of an existing file.
        let file = options.open(&path)?;
        let mut temporary = TemporaryDiagnosticFile {
            path: Some(path.clone()),
            file: Some(file),
        };
        let result = (|| {
            const CONTENT: &[u8] = b"ClipClub temporary storage diagnostic\n";
            let file = temporary.file.as_mut().unwrap();
            file.write_all(CONTENT)?;
            file.sync_all()?;
            file.rewind()?;
            let mut actual = [0; CONTENT.len()];
            file.read_exact(&mut actual)?;
            if actual != CONTENT {
                return Err(io::Error::other("Storage round trip did not match"));
            }
            Ok(())
        })();
        drop(temporary.file.take());
        let cleanup = std::fs::remove_file(&path);
        if cleanup.is_ok() {
            temporary.path = None;
        }
        result.and(cleanup)
    });
    match tokio::time::timeout(PROBE_TIMEOUT, worker).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(io::Error::other("Storage worker could not finish")),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Storage test timed out",
        )),
    }
}

async fn device_checks(context: &DiagnosticContext, checks: &mut Vec<DiagnosticCheck>) {
    let root = context.root.clone();
    let capacity = match tokio::time::timeout(
        PROBE_TIMEOUT,
        tokio::task::spawn_blocking(move || fs2::available_space(root)),
    )
    .await
    {
        Ok(Ok(result)) => result,
        _ => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Storage capacity query did not finish",
        )),
    };
    checks.push(match capacity {
        Ok(bytes) => DiagnosticCheck::new(
            "device",
            "Available profile storage",
            if bytes < 128 * 1024 * 1024 { "warning" } else { "pass" },
            format!(
                "{} MiB available on the profile volume. Low free space (below 128 MiB) can interrupt downloads or uploads; this is a snapshot and does not reserve space.",
                bytes / (1024 * 1024)
            ),
        ),
        Err(_) => DiagnosticCheck::new(
            "device", "Available profile storage", "warning",
            "The available space on the profile volume could not be read.",
        ),
    });
    let mut tests = Vec::new();
    for (folder, label) in [
        ("", "Profile storage access"),
        ("clips", "Library storage access"),
        ("cache", "Playback cache storage access"),
        ("incoming", "Incoming storage access"),
        ("roll-cache", "Roll cache storage access"),
    ] {
        tests.push(async move {
            let result = storage_round_trip(&context.root.join(folder)).await;
            (label, result)
        });
    }
    let results = stream::iter(tests).buffered(5).collect::<Vec<_>>().await;
    for (label, result) in results {
        checks.push(DiagnosticCheck::new(
            "device",
            label,
            if result.is_ok() { "pass" } else { "fail" },
            if result.is_ok() {
                "Created, wrote, synced, read, and deleted a small temporary test file. Saved application files were not opened."
            } else {
                "A temporary file could not complete the create/write/read/delete test. Check available storage and this device's profile folder permissions. A timed-out worker continues cleanup when storage responds; a failed cleanup may leave a small .clipclub-diagnostic file."
            },
        ));
    }
}

async fn validator_version(probe: &Path, timeout: Duration) -> Result<String, &'static str> {
    let mut child = Command::new(probe)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "The video validator could not be started. Reinstall this device's matching ClipClub build and check executable permissions.")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("The video validator did not provide version output.")?;
    let result = tokio::time::timeout(timeout, async {
        // Do not capture arbitrary executable output in a diagnostic report.
        let mut output = Vec::new();
        let mut stdout = stdout.take(4096);
        let (read, status) = tokio::join!(stdout.read_to_end(&mut output), child.wait());
        let status =
            status.map_err(|_| "The video validator could not finish its version check.")?;
        read.map_err(|_| "The video validator version output could not be read.")?;
        if !status.success() {
            return Err("The video validator exited unsuccessfully during its version check.");
        }
        let text = String::from_utf8_lossy(&output);
        let version = text
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("ffprobe version "))
            .and_then(|value| value.split_whitespace().next())
            .map(|value| {
                // Vendor builds can append a website or build path to a
                // numeric release. Keep only its release number in reports.
                if value.starts_with(|character: char| character.is_ascii_digit()) {
                    value
                        .split(|character: char| !character.is_ascii_digit() && character != '.')
                        .next()
                        .unwrap_or_default()
                        .trim_end_matches('.')
                } else {
                    value
                }
            })
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 80
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
            })
            .ok_or("The video validator returned an unrecognized version response.")?;
        Ok(version.to_owned())
    })
    .await;
    result.unwrap_or(Err(
        "The video validator did not finish within three seconds. The process was stopped.",
    ))
}

async fn media_checks(context: &DiagnosticContext, checks: &mut Vec<DiagnosticCheck>) {
    checks.push(match validator_version(&context.probe, PROBE_TIMEOUT).await {
        Ok(version) => DiagnosticCheck::new(
            "media", "Video validator executable", "pass",
            format!("ffprobe {version} started and answered on this device. This verifies the validator executable, not playback or decoding in the app's video view."),
        ),
        Err(detail) => DiagnosticCheck::new("media", "Video validator executable", "fail", detail),
    });
    checks.push(DiagnosticCheck::new(
        "media",
        "Clips and downloads",
        if context.recent_download_failures > 0 { "warning" } else { "info" },
        format!(
            "{} locally seeded clip(s), {} known catalog clip(s), {} ready to view, {} recent failed download(s). {}Remote clips enter the viewing queue only after downloading and validating the video. Receiving metadata does not establish video-port reachability.",
            context.local_seeded_clips, context.catalog_clips, context.ready_clips,
            context.recent_download_failures,
            context.downloading.map(|(downloaded, total)| format!("Current download: {downloaded}/{total} bytes. ")).unwrap_or_default(),
        ),
    ));
    checks.push(DiagnosticCheck::new(
        "media", "Playback session and cache", "info",
        format!(
            "Rating playback session is {}; {} temporary clip(s) currently managed in the playback cache. Open Rate to view the queue. Downloads prefetch while the app is open. Check actual picture, audio, seeking, and resizing on each device using the same MP4 and WebM clips.",
            if context.rating_session_active { "active" } else { "inactive" }, context.cached_clips,
        ),
    ));
    checks.push(DiagnosticCheck::new(
        "media", "Accepted video limits", "info",
        format!(
            "MP4 and WebM; up to {} bytes, {} seconds, and {} pixels per dimension. A successful upload validation does not guarantee this device's webview supports the video's codec.",
            crate::core::MAX_FILE_BYTES, crate::core::MAX_DURATION_SECONDS, crate::core::MAX_DIMENSION,
        ),
    ));
}

async fn network_checks(
    context: &DiagnosticContext,
    mesh: &Result<PeerDiagnostics>,
    request: &DiagnosticRequest,
    checks: &mut Vec<DiagnosticCheck>,
) {
    match mesh {
        Ok(mesh) => {
            checks.push(DiagnosticCheck::new(
                "network",
                "mDNS local discovery",
                if mesh.mdns_enabled && !mesh.mdns_endpoints.is_empty() {
                    "pass"
                } else {
                    "warning"
                },
                if !mesh.mdns_requested {
                    "Local mDNS discovery is disabled for this process.".into()
                } else if !mesh.mdns_enabled {
                    "Local mDNS discovery could not be initialized. Internet discovery may still work.".into()
                } else {
                    format!(
                        "mDNS behaviour enabled; {} current remote endpoint(s) observed by the running app. This is a snapshot, not an independent multicast packet test or proof that every interface could bind its discovery socket. If none are listed, keep both apps open, wait at least 15 seconds, and rerun; an empty result alone does not locate a firewall or Wi-Fi problem.",
                        mesh.mdns_endpoints.len()
                    )
                },
            ));
            checks.push(DiagnosticCheck::new(
                "network",
                "Authenticated peer mesh",
                if mesh.authenticated_peers.is_empty() { "warning" } else { "pass" },
                format!(
                    "{} authenticated peer(s); {} peer(s) subscribed to ClipClub; {} current seeder presence record(s). One established mesh connection carries metadata both ways.",
                    mesh.authenticated_peers.len(), mesh.subscribed_peers, context.connected_presence
                ),
            ));
            checks.push(DiagnosticCheck::new(
                "network",
                "Peer messages since app startup",
                "info",
                format!(
                    "{} validated clip announcement(s), {} validated seeder announcement(s), {} rejected message(s). These counts describe received metadata, not successful video downloads.",
                    mesh.received_adverts, mesh.received_presence, mesh.rejected_messages
                ),
            ));
            checks.push(DiagnosticCheck::new(
                "network",
                "Internet discovery",
                "info",
                if mesh.internet_discovery_running {
                    "DHT discovery worker is running. External DHT reachability, router mappings, and inbound internet reachability were not tested."
                } else {
                    "DHT discovery worker is not currently running. This does not prevent local mDNS discovery."
                },
            ));
        }
        Err(error) => checks.push(DiagnosticCheck::new(
            "network",
            "Peer mesh snapshot",
            "fail",
            format!("Could not inspect the running mesh: {error:#}"),
        )),
    }
    let targets = probes(context, mesh.as_ref().ok(), request);
    if targets.iter().all(|target| target.local) {
        checks.push(DiagnosticCheck::new(
            "network",
            "Other device TCP tests",
            "warning",
            "No remote endpoint is available to test. Enter the other device's IPv4 address to test its peer mesh and video ports even when discovery fails.",
        ));
    }
    let outcomes = stream::iter(
        targets
            .into_iter()
            .enumerate()
            .map(|(index, probe)| async move {
                let started = Instant::now();
                let result = connect_result(TcpStream::connect(probe.address), PROBE_TIMEOUT).await;
                (index, probe, result, started.elapsed())
            }),
    )
    .buffer_unordered(PROBE_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;
    let failed_video = outcomes.iter().any(|(_, probe, result, _)| {
        !probe.local && probe.video && *result != Reachability::Reachable
    });
    let mut outcomes = outcomes;
    outcomes.sort_by_key(|(index, _, _, _)| *index);
    checks.extend(
        outcomes
            .iter()
            .map(|(_, probe, result, elapsed)| probe_check(probe, result, *elapsed)),
    );
    if failed_video {
        checks.push(DiagnosticCheck::new(
            "network",
            "Video transfer finding",
            "warning",
            "At least one remote video port is unreachable from this device. Its clips may be discovered but never appear in the viewing queue. Run diagnostics on both devices and compare directions before changing firewall rules.",
        ));
    }
}

/// Runs only the requested suites with bounded storage, validator, and TCP
/// checks. Reports remain useful when the mesh cannot provide a snapshot.
pub async fn run(
    mut context: DiagnosticContext,
    mesh: Result<PeerDiagnostics>,
    request: DiagnosticRequest,
) -> Result<DiagnosticReport> {
    request.validate()?;
    // The CLI permits port zero, letting the OS choose a mesh port. The
    // process's original option remains zero; test/report its actual listener.
    if context.p2p_port == 0
        && let Ok(mesh) = &mesh
        && let Some(port) = mesh.listen_addresses.iter().find_map(|address| {
            address
                .parse::<Multiaddr>()
                .ok()?
                .iter()
                .find_map(|part| match part {
                    Protocol::Tcp(port) if port != 0 => Some(port),
                    _ => None,
                })
        })
    {
        context.p2p_port = port;
    }
    let generated_at_ms = crate::core::current_time_ms();
    let mut checks = Vec::new();
    checks.push(DiagnosticCheck::new(
        if request.suite == DiagnosticSuite::All {
            "device"
        } else {
            request.suite.name()
        },
        "Application",
        "info",
        format!(
            "ClipClub {} · {}/{} · peer {} · peer port {} · video port {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            context.peer_id,
            if context.p2p_port == 0 {
                "automatic (see Discovery & peers)".into()
            } else {
                context.p2p_port.to_string()
            },
            context.torrent_port
        ),
    ));
    if request.suite.includes(DiagnosticSuite::Device) {
        device_checks(&context, &mut checks).await;
    }
    if request.suite.includes(DiagnosticSuite::Media) {
        media_checks(&context, &mut checks).await;
    }
    if request.suite.includes(DiagnosticSuite::Network) {
        network_checks(&context, &mesh, &request, &mut checks).await;
    }
    if request.suite.includes(DiagnosticSuite::Wallet) {
        checks.append(&mut context.wallet_checks);
    }
    let failures = checks.iter().filter(|check| check.status == "fail").count();
    let warnings = checks
        .iter()
        .filter(|check| check.status == "warning")
        .count();
    let summary = if failures > 0 {
        format!(
            "{failures} check(s) failed; {warnings} warning(s). Compare reports from both devices."
        )
    } else if warnings > 0 {
        format!("Checks completed with {warnings} warning(s). Review the observations below.")
    } else {
        "Checks completed. Review each observation and finish the physical-device checklist.".into()
    };
    let mut report = format!(
        "ClipClub diagnostics\nSuite: {}\nReport time (Unix milliseconds): {generated_at_ms}\n{summary}\n\n",
        request.suite.name()
    );
    for check in &checks {
        report.push_str(&format!(
            "[{}] {} / {}\n{}\n\n",
            check.status.to_uppercase(),
            check.category,
            check.name,
            check.detail
        ));
    }
    if request.suite.includes(DiagnosticSuite::Network)
        && let Ok(mesh) = &mesh
    {
        report.push_str("Current mesh listen addresses:\n");
        for address in &mesh.listen_addresses {
            report.push_str(&format!("  {address}\n"));
        }
        if mesh.listen_addresses.is_empty() {
            report.push_str("  none\n");
        }
        report.push_str("\nCurrent mDNS observations (unverified discovery candidates):\n");
        for address in &mesh.mdns_endpoints {
            report.push_str(&format!("  {address}\n"));
        }
        if mesh.mdns_endpoints.is_empty() {
            report.push_str("  none\n");
        }
        report.push_str("\nAuthenticated peer identities:\n");
        for peer in &mesh.authenticated_peers {
            report.push_str(&format!("  {peer}\n"));
        }
        if mesh.authenticated_peers.is_empty() {
            report.push_str("  none\n");
        }
        report.push_str(
            "\nRecent mesh dial errors (bounded history; stale discovery results can be normal):\n",
        );
        for error in &mesh.recent_dial_errors {
            report.push_str(&format!("  {}: {}\n", error.at_ms, error.detail));
        }
        if mesh.recent_dial_errors.is_empty() {
            report.push_str("  none recorded\n");
        }
    }
    report.push_str("\nThese checks do not change firewall/router settings, verify remote protocols, or measure reachability in the reverse direction. Run this report on both devices. Reports contain peer identities and IP addresses; no wallet secrets or profile contents are included.\n");
    Ok(DiagnosticReport {
        suite: request.suite,
        generated_at_ms,
        summary,
        checks,
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> DiagnosticContext {
        DiagnosticContext {
            root: PathBuf::from("/missing-clipclub-diagnostic-test-root"),
            probe: PathBuf::from("/missing-clipclub-diagnostic-test-ffprobe"),
            cached_clips: 0,
            rating_session_active: false,
            wallet_checks: vec![],
            peer_id: "local-peer".into(),
            p2p_port: 42931,
            torrent_port: 42932,
            local_seeded_clips: 1,
            catalog_clips: 2,
            ready_clips: 0,
            connected_presence: 1,
            peer_video_endpoints: vec![],
            downloading: None,
            recent_download_failures: 0,
        }
    }

    #[test]
    fn target_validation_rejects_invalid_addresses_ports_and_unknown_fields() {
        for address in ["0.0.0.0", "224.0.0.251", "255.255.255.255"] {
            let request = DiagnosticRequest {
                peer_ip: Some(address.parse().unwrap()),
                ..Default::default()
            };
            assert!(request.validate().is_err());
        }
        assert!(
            DiagnosticRequest {
                p2p_port: Some(0),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            DiagnosticRequest {
                torrent_port: Some(0),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        for input in [
            r#"{"peer_ip":"example.com"}"#,
            r#"{"p2p_port":65536}"#,
            r#"{"command":"shell"}"#,
            r#"{"suite":"unknown"}"#,
        ] {
            assert!(serde_json::from_str::<DiagnosticRequest>(input).is_err());
        }
    }

    #[tokio::test]
    async fn focused_suites_do_not_probe_network_or_include_other_categories() {
        for suite in [
            DiagnosticSuite::Device,
            DiagnosticSuite::Media,
            DiagnosticSuite::Wallet,
        ] {
            let mut context = context();
            context.p2p_port = 0;
            context.wallet_checks.push(DiagnosticCheck::new(
                "wallet",
                "Wallet observation",
                "info",
                "Read only",
            ));
            let report = run(
                context,
                Err(anyhow::anyhow!(
                    "private error text must not reach another suite"
                )),
                DiagnosticRequest {
                    suite,
                    peer_ip: Some(Ipv4Addr::LOCALHOST),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(report.suite, suite);
            assert!(
                report
                    .report
                    .contains("peer port automatic (see Discovery & peers)")
            );
            assert!(
                report
                    .checks
                    .iter()
                    .all(|check| check.category == suite.name())
            );
            assert!(!report.report.contains("private error text"));
            assert!(!report.report.contains("Current mesh listen addresses"));
            assert!(!report.checks.iter().any(
                |check| check.name.contains("listener") || check.name.contains("Other device")
            ));
            assert!(!report.report.contains("/missing-clipclub"));
            assert!(
                report
                    .checks
                    .iter()
                    .any(|check| check.name == "Wallet observation")
                    == (suite == DiagnosticSuite::Wallet)
            );
        }
    }

    #[tokio::test]
    async fn storage_checks_preserve_saved_files_and_remove_temporary_files() {
        let directory = tempfile::tempdir().unwrap();
        let mut context = context();
        context.root = directory.path().to_owned();
        for folder in ["", "clips", "cache", "incoming", "roll-cache"] {
            let path = directory.path().join(folder);
            tokio::fs::create_dir_all(&path).await.unwrap();
            tokio::fs::write(path.join("saved-file"), b"preserve me")
                .await
                .unwrap();
        }
        let mut checks = vec![];
        device_checks(&context, &mut checks).await;
        assert_eq!(checks.len(), 6);
        assert!(
            checks
                .iter()
                .all(|check| check.status == "pass" || check.name == "Available profile storage")
        );
        for folder in ["", "clips", "cache", "incoming", "roll-cache"] {
            let path = directory.path().join(folder);
            assert_eq!(
                tokio::fs::read(path.join("saved-file")).await.unwrap(),
                b"preserve me"
            );
            let entries: Vec<_> = std::fs::read_dir(path)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert!(
                entries
                    .iter()
                    .all(|name| !name.to_string_lossy().starts_with(".clipclub-diagnostic-"))
            );
        }
    }

    #[tokio::test]
    async fn storage_errors_are_actionable_without_exposing_local_paths() {
        let directory = tempfile::tempdir().unwrap();
        let mut context = context();
        // A regular file cannot serve as a profile directory, even when tests run as root.
        context.root = directory.path().join("private-profile-name");
        tokio::fs::write(&context.root, b"preserve me")
            .await
            .unwrap();
        let mut checks = vec![];
        device_checks(&context, &mut checks).await;
        assert_eq!(
            checks.iter().filter(|check| check.status == "fail").count(),
            5
        );
        assert!(
            checks
                .iter()
                .all(|check| !check.detail.contains("private-profile-name"))
        );
        assert_eq!(tokio::fs::read(context.root).await.unwrap(), b"preserve me");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn validator_check_runs_bounded_processes_and_reports_only_version() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let probe = directory.path().join("private-executable-name");
        async fn script(path: &Path, body: &str) {
            tokio::fs::write(path, format!("#!/bin/sh\n{body}\n"))
                .await
                .unwrap();
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .await
                .unwrap();
        }
        script(
            &probe,
            "printf 'ffprobe version 9.1.2-test Copyright\\nprivate config path\\n'",
        )
        .await;
        assert_eq!(
            validator_version(&probe, PROBE_TIMEOUT).await.unwrap(),
            "9.1.2"
        );
        script(
            &probe,
            "printf 'ffprobe version 9.0.2-https://www.martin-riedl.de Copyright\\nprivate config path\\n'",
        )
        .await;
        assert_eq!(
            validator_version(&probe, PROBE_TIMEOUT).await.unwrap(),
            "9.0.2"
        );
        script(&probe, "printf 'ffprobe version /private/path\\n'").await;
        assert!(
            validator_version(&probe, PROBE_TIMEOUT)
                .await
                .unwrap_err()
                .contains("unrecognized")
        );
        script(&probe, "exit 2").await;
        assert!(
            validator_version(&probe, PROBE_TIMEOUT)
                .await
                .unwrap_err()
                .contains("unsuccessfully")
        );
        script(&probe, "exec sleep 30").await;
        let start = Instant::now();
        assert!(
            validator_version(&probe, Duration::from_millis(30))
                .await
                .unwrap_err()
                .contains("did not finish")
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(
            !validator_version(
                &directory.path().join("missing-private-executable"),
                PROBE_TIMEOUT
            )
            .await
            .unwrap_err()
            .contains("private")
        );
    }

    #[test]
    fn automatic_probes_are_bounded_and_exclude_unverified_public_mdns_targets() {
        let mesh = PeerDiagnostics {
            mdns_endpoints: (1..=100)
                .map(|port| format!("/ip4/192.168.1.20/tcp/{port}"))
                .chain([
                    "/ip4/8.8.8.8/tcp/53".into(),
                    "/ip4/127.0.0.1/tcp/1234".into(),
                    "/ip4/192.168.1.20/tcp/1".into(),
                ])
                .collect(),
            ..Default::default()
        };
        let request = DiagnosticRequest {
            peer_ip: Some("192.168.1.88".parse().unwrap()),
            p2p_port: Some(15000),
            torrent_port: Some(15001),
            ..Default::default()
        };
        let targets = probes(&context(), Some(&mesh), &request);
        assert_eq!(targets.len(), MAX_REMOTE_PROBES + 2);
        assert_eq!(targets[2].address, "192.168.1.88:15000".parse().unwrap());
        assert_eq!(targets[3].address, "192.168.1.88:15001".parse().unwrap());
        assert!(
            targets
                .iter()
                .skip(2)
                .all(|target| local_network(target.address.ip()))
        );
        let unique: HashSet<_> = targets
            .iter()
            .skip(2)
            .map(|target| (target.address, target.video))
            .collect();
        assert_eq!(unique.len(), MAX_REMOTE_PROBES);
    }

    #[tokio::test]
    async fn tcp_probe_distinguishes_reachable_refused_and_timeout() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        assert_eq!(
            connect_result(TcpStream::connect(address), PROBE_TIMEOUT).await,
            Reachability::Reachable
        );
        let (accepted, _) = listener.accept().await.unwrap();
        drop(accepted);
        drop(listener);
        assert_eq!(
            connect_result(TcpStream::connect(address), PROBE_TIMEOUT).await,
            Reachability::Refused
        );
        assert_eq!(
            connect_result(std::future::pending(), Duration::from_millis(1)).await,
            Reachability::TimedOut
        );
    }

    #[tokio::test]
    async fn report_explains_video_failure_without_claiming_discovery_failure() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let closed = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        drop(closed);
        let mut context = context();
        context.p2p_port = port;
        context.torrent_port = port;
        let request = DiagnosticRequest {
            peer_ip: Some(Ipv4Addr::LOCALHOST),
            p2p_port: Some(port),
            torrent_port: Some(closed_port),
            ..Default::default()
        };
        let mesh = PeerDiagnostics {
            mdns_requested: true,
            mdns_enabled: true,
            authenticated_peers: vec!["remote-peer".into()],
            received_adverts: 1,
            ..Default::default()
        };
        let report = run(context, Ok(mesh), request).await.unwrap();
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.name == "Video transfer finding" && check.status == "warning")
        );
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.name == "Authenticated peer mesh" && check.status == "pass")
        );
        assert!(
            report
                .report
                .contains("Remote clips enter the viewing queue only after downloading")
        );
        assert!(
            report
                .report
                .contains("application protocol and video contents were not checked")
        );
        assert!(
            report
                .report
                .contains("not an independent multicast packet test")
        );
    }

    #[tokio::test]
    async fn ephemeral_mesh_port_is_resolved_from_the_running_listener() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut context = context();
        context.p2p_port = 0;
        context.torrent_port = port;
        let mesh = PeerDiagnostics {
            listen_addresses: vec![format!("/ip4/127.0.0.1/tcp/{port}")],
            ..Default::default()
        };
        let report = run(context, Ok(mesh), DiagnosticRequest::default())
            .await
            .unwrap();
        assert!(report.checks.iter().any(|check| check.name
            == format!("Local peer mesh listener · 127.0.0.1:{port}")
            && check.status == "pass"));
        assert!(report.report.contains(&format!("peer port {port}")));
    }
}
