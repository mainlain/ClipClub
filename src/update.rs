//! Anonymous GitHub release checks. Only a verified, matching stable release
//! can reach the native installer; no GitHub credential is shipped with the app.
use anyhow::{Context, Result, ensure};
use futures::StreamExt;
use reqwest::{Client, StatusCode};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncWriteExt, sync::Mutex};

const API: &str = "https://api.github.com/repos/mainlain/ClipClub/releases/latest";
const REPO: &str = "https://github.com/mainlain/ClipClub";
const MAX_METADATA: usize = 2 * 1024 * 1024;
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub current_version: &'static str,
    pub latest_version: Option<String>,
    pub available: bool,
    pub release_url: Option<String>,
    pub download_url: Option<String>,
    pub status: &'static str,
    pub phase: &'static str,
    pub can_install: bool,
    pub error: Option<String>,
    pub checked_at: Option<u64>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            current_version: env!("CARGO_PKG_VERSION"),
            latest_version: None,
            available: false,
            release_url: None,
            download_url: None,
            status: "idle",
            phase: "idle",
            can_install: false,
            error: None,
            checked_at: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    state: String,
    digest: Option<String>,
}

#[derive(Clone, Debug)]
struct Candidate {
    version: Version,
    release_url: String,
    asset: Option<Asset>,
}

struct State {
    status: Status,
    candidate: Option<Candidate>,
    last_attempt: Option<Instant>,
}

pub struct Updater {
    state: Mutex<State>,
    root: PathBuf,
}

impl Updater {
    pub fn new(root: PathBuf) -> Self {
        // An interrupted download never becomes an installable archive.
        // The profile lock ensures no other client is using these scratch files.
        if let Ok(entries) = std::fs::read_dir(root.join("updates")) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("download-") && name.ends_with(".partial") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        Self {
            state: Mutex::new(State {
                status: Status::default(),
                candidate: None,
                last_attempt: None,
            }),
            root,
        }
    }

    pub async fn status(&self) -> Status {
        self.state.lock().await.status.clone()
    }

    pub async fn check(&self) -> Status {
        {
            let mut state = self.state.lock().await;
            // Serialize checks and bound anonymous API use even if repeatedly clicked.
            if state.status.phase != "idle"
                || state.status.status == "checking"
                || state
                    .last_attempt
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
            {
                return state.status.clone();
            }
            state.last_attempt = Some(Instant::now());
            state.status.status = "checking";
            state.status.error = None;
        }
        let result = fetch_release().await;
        let mut state = self.state.lock().await;
        state.status.checked_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );
        match result {
            Ok(candidate) => {
                let mut status = Status {
                    checked_at: state.status.checked_at,
                    ..Status::default()
                };
                status.status = "unavailable";
                if let Some(candidate) = &candidate {
                    status.available = candidate
                        .version
                        .cmp_precedence(&Version::parse(env!("CARGO_PKG_VERSION")).unwrap())
                        .is_gt();
                    status.status = if status.available {
                        "available"
                    } else {
                        "current"
                    };
                    status.latest_version = Some(candidate.version.to_string());
                    status.release_url = Some(candidate.release_url.clone());
                    status.download_url = candidate
                        .asset
                        .as_ref()
                        .map(|a| a.browser_download_url.clone());
                    status.can_install = status.available
                        && candidate
                            .asset
                            .as_ref()
                            .is_some_and(|a| sha256_digest(a).is_ok())
                        && crate::update_install::can_install();
                }
                state.status = status;
                state.candidate = candidate;
            }
            Err(error) => {
                // Keep an earlier successful result usable during an outage.
                state.status.status = "unavailable";
                state.status.error = Some(format!("Could not check GitHub releases: {error:#}"));
            }
        }
        state.status.clone()
    }

    pub async fn open(&self, target: &str) -> Result<()> {
        let url = {
            let state = self.state.lock().await;
            match target {
                "release" => state
                    .status
                    .release_url
                    .clone()
                    .or_else(|| Some(format!("{REPO}/releases"))),
                "download" => state.status.download_url.clone(),
                _ => anyhow::bail!("Unknown update link"),
            }
            .context("No download is available for this platform yet")?
        };
        // URLs come only from validated, fixed repository metadata, never client input.
        open_browser(&url)
    }

    pub async fn install(
        self: &Arc<Self>,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<Status> {
        let candidate = {
            let mut state = self.state.lock().await;
            ensure!(
                state.status.phase == "idle",
                "An update is already in progress"
            );
            ensure!(
                state.status.status != "checking",
                "Wait for the release check to finish"
            );
            ensure!(
                state.status.available && state.status.can_install,
                "Use Download update to install this release manually"
            );
            let candidate = state.candidate.clone().context("Check for updates first")?;
            state.status.phase = "downloading";
            state.status.error = None;
            candidate
        };
        let updater = self.clone();
        tokio::spawn(async move {
            let result = updater.prepare_and_launch(candidate).await;
            match result {
                Ok(()) => {
                    updater.state.lock().await.status.phase = "installing";
                    // Allow the local API response and polling UI to finish before quitting.
                    tokio::time::sleep(Duration::from_millis(750)).await;
                    shutdown.cancel();
                }
                Err(error) => {
                    let mut state = updater.state.lock().await;
                    state.status.phase = "idle";
                    state.status.error = Some(format!(
                        "Update failed; the installed app is unchanged: {error:#}"
                    ));
                }
            }
        });
        Ok(self.status().await)
    }

    async fn prepare_and_launch(&self, candidate: Candidate) -> Result<()> {
        let asset = candidate.asset.context("Missing platform download")?;
        let directory = self.root.join("updates");
        tokio::fs::create_dir_all(&directory).await?;
        let archive = directory.join(format!(
            "download-{}.partial",
            hex::encode(rand::random::<[u8; 12]>())
        ));
        let result = async {
            download(&asset, &archive).await?;
            let archive_path = archive.clone();
            let profile = self.root.clone();
            let version = candidate.version.to_string();
            tokio::task::spawn_blocking(move || {
                crate::update_install::prepare(&archive_path, &version, &profile)?.launch()
            })
            .await??;
            Ok(())
        }
        .await;
        let _ = tokio::fs::remove_file(&archive).await;
        result
    }
}

fn platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("macos-aarch64.zip"),
        ("macos", "x86_64") => Some("macos-x86_64.zip"),
        ("linux", "x86_64") => Some("linux-x86_64.tar.gz"),
        _ => None,
    }
}

fn select_release(
    release: Release,
    current: &Version,
    platform: Option<&str>,
) -> Result<Option<Candidate>> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .context("Release tag must be a semantic version, such as v1.2.3")?;
    if !version.pre.is_empty() {
        return Ok(None);
    }
    let release_url = format!("{REPO}/releases/tag/{}", release.tag_name);
    let expected_name = platform.map(|platform| format!("clipclub-{version}-{platform}"));
    let asset = release.assets.into_iter().find(|asset| {
        Some(&asset.name) == expected_name.as_ref()
            && asset.state == "uploaded"
            && asset.size > 0
            && asset.size <= MAX_DOWNLOAD
            && asset.browser_download_url
                == format!(
                    "{REPO}/releases/download/{}/{}",
                    release.tag_name, asset.name
                )
    });
    // Never offer an older published release as an installation target.
    Ok(Some(Candidate {
        version,
        release_url,
        asset: if version_is_newer(&release.tag_name, current) {
            asset
        } else {
            None
        },
    }))
}

fn version_is_newer(tag: &str, current: &Version) -> bool {
    Version::parse(tag.strip_prefix('v').unwrap_or(tag))
        .is_ok_and(|version| version.cmp_precedence(current).is_gt())
}

fn client(download: bool) -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!("ClipClub/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(if download { 600 } else { 20 }))
        .redirect(if download {
            reqwest::redirect::Policy::custom(|attempt| {
                let allowed = matches!(
                    attempt.url().host_str(),
                    Some(
                        "github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                    )
                );
                if allowed && attempt.previous().len() < 5 && attempt.url().scheme() == "https" {
                    attempt.follow()
                } else {
                    attempt.error("Untrusted release download redirect")
                }
            })
        } else {
            reqwest::redirect::Policy::none()
        })
        .build()?)
}

async fn fetch_release() -> Result<Option<Candidate>> {
    let response = client(false)?
        .get(API)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .context("GitHub is unreachable")?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    ensure!(
        response.status().is_success(),
        "GitHub returned {} (try again later)",
        response.status()
    );
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        ensure!(
            bytes.len() + chunk.len() <= MAX_METADATA,
            "Release metadata is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    select_release(
        serde_json::from_slice(&bytes)?,
        &Version::parse(env!("CARGO_PKG_VERSION"))?,
        platform(),
    )
}

fn sha256_digest(asset: &Asset) -> Result<Vec<u8>> {
    let text = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .context("GitHub did not supply a SHA-256 digest; use the manual download")?;
    ensure!(text.len() == 64, "Invalid release checksum");
    hex::decode(text).context("Invalid release checksum")
}

async fn download(asset: &Asset, path: &Path) -> Result<()> {
    let response = client(true)?
        .get(&asset.browser_download_url)
        .send()
        .await?
        .error_for_status()?;
    store_download(response, asset, path).await
}

async fn store_download(response: reqwest::Response, asset: &Asset, path: &Path) -> Result<()> {
    let expected = sha256_digest(asset)?;
    if let Some(size) = response.content_length() {
        ensure!(size == asset.size, "Release download size changed");
    }
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    let mut hash = Sha256::new();
    let mut total = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        total += chunk.len() as u64;
        ensure!(
            total <= asset.size && total <= MAX_DOWNLOAD,
            "Release download exceeded its expected size"
        );
        hash.update(&chunk);
        file.write_all(&chunk).await?;
    }
    ensure!(total == asset.size, "Release download is incomplete");
    ensure!(
        hash.finalize().as_slice() == expected,
        "Release checksum mismatch"
    );
    file.sync_all().await?;
    Ok(())
}

pub(crate) fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("/usr/bin/open");
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    anyhow::bail!("Open {url} in your browser");
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        command
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Could not open the system browser")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> Release {
        serde_json::from_value(serde_json::json!({
            "tag_name":tag,"draft":false,"prerelease":false,
            "assets":[{"name":format!("clipclub-{}-macos-aarch64.zip", tag.trim_start_matches('v')),
                "browser_download_url":format!("{REPO}/releases/download/{tag}/clipclub-{}-macos-aarch64.zip",tag.trim_start_matches('v')),
                "size":42,"state":"uploaded","digest":format!("sha256:{}","ab".repeat(32))}]
        })).unwrap()
    }

    #[test]
    fn compares_semantic_versions_and_never_downgrades() {
        let current = Version::parse("0.9.0").unwrap();
        let newer = select_release(release("v0.10.0"), &current, Some("macos-aarch64.zip"))
            .unwrap()
            .unwrap();
        assert!(newer.asset.is_some());
        for tag in ["v0.8.0", "v0.9.0", "v0.9.0+build.2"] {
            assert!(
                select_release(release(tag), &current, Some("macos-aarch64.zip"))
                    .unwrap()
                    .unwrap()
                    .asset
                    .is_none()
            );
        }
        for tag in ["v1.0.0-beta.1", "v1.0.0-rc.1"] {
            assert!(
                select_release(release(tag), &current, Some("macos-aarch64.zip"))
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn filters_drafts_prereleases_wrong_platform_and_external_urls() {
        let current = Version::parse("0.1.0").unwrap();
        for field in ["draft", "prerelease"] {
            let mut r = release("v0.2.0");
            if field == "draft" {
                r.draft = true;
            } else {
                r.prerelease = true;
            }
            assert!(
                select_release(r, &current, Some("macos-aarch64.zip"))
                    .unwrap()
                    .is_none()
            );
        }
        assert!(
            select_release(release("v0.2.0"), &current, Some("linux-x86_64.tar.gz"))
                .unwrap()
                .unwrap()
                .asset
                .is_none()
        );
        let mut r = release("v0.2.0");
        r.assets[0].browser_download_url =
            "https://github.com/attacker/ClipClub/releases/download/v0.2.0/update.zip".into();
        assert!(
            select_release(r, &current, Some("macos-aarch64.zip"))
                .unwrap()
                .unwrap()
                .asset
                .is_none()
        );
    }

    #[test]
    fn requires_exact_sha256_and_bounded_uploaded_asset() {
        let mut r = release("v0.2.0");
        assert_eq!(sha256_digest(&r.assets[0]).unwrap(), vec![0xab; 32]);
        for digest in [
            None,
            Some("sha256:abc".to_owned()),
            Some(format!("sha256:{}", "zz".repeat(32))),
        ] {
            r.assets[0].digest = digest;
            assert!(sha256_digest(&r.assets[0]).is_err());
        }
        r.assets[0].size = MAX_DOWNLOAD + 1;
        assert!(
            select_release(
                r,
                &Version::parse("0.1.0").unwrap(),
                Some("macos-aarch64.zip")
            )
            .unwrap()
            .unwrap()
            .asset
            .is_none()
        );
    }

    #[tokio::test]
    async fn downloaded_bytes_must_match_size_and_checksum() {
        let temp = tempfile::tempdir().unwrap();
        let mut asset = release("v0.2.0").assets.remove(0);
        asset.size = 7;
        asset.digest = Some(format!(
            "sha256:{}",
            hex::encode(Sha256::digest(b"release"))
        ));
        let response = || reqwest::Response::from(axum::http::Response::new("release"));
        let path = temp.path().join("good");
        store_download(response(), &asset, &path).await.unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"release");
        asset.digest = Some(format!("sha256:{}", "00".repeat(32)));
        assert!(
            store_download(response(), &asset, &temp.path().join("corrupt"))
                .await
                .is_err()
        );
        asset.size = 6;
        assert!(
            store_download(response(), &asset, &temp.path().join("long"))
                .await
                .is_err()
        );
        asset.size = 8;
        assert!(
            store_download(response(), &asset, &temp.path().join("short"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_check_in_flight_cannot_race_an_install() {
        let temp = tempfile::tempdir().unwrap();
        let updater = Arc::new(Updater::new(temp.path().to_owned()));
        {
            let mut state = updater.state.lock().await;
            state.status.available = true;
            state.status.can_install = true;
            state.status.status = "checking";
            state.candidate = select_release(
                release("v0.2.0"),
                &Version::parse("0.1.0").unwrap(),
                Some("macos-aarch64.zip"),
            )
            .unwrap();
        }
        let shutdown = tokio_util::sync::CancellationToken::new();
        assert!(
            updater
                .install(shutdown.clone())
                .await
                .unwrap_err()
                .to_string()
                .contains("release check")
        );
        assert_eq!(updater.status().await.phase, "idle");
        assert!(!shutdown.is_cancelled());
    }
}
