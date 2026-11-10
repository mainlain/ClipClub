//! Opt-in delivery of diagnostic reports, independent of the peer network.
//! Only explicitly queued report text is sent; profile files are never attached.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

pub const MAX_REPORT_BYTES: usize = 240 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_PENDING: usize = 100;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    enabled: bool,
    endpoint: String,
    upload_token: String,
    device: String,
    session: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configure {
    pub enabled: bool,
    pub endpoint: String,
    #[serde(default)]
    pub upload_token: Option<String>,
    pub device: String,
    pub session: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportInput {
    pub suite: String,
    pub generated_at_ms: u64,
    pub report: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Report {
    schema_version: u8,
    id: String,
    generated_at_ms: u64,
    device: String,
    session: String,
    suite: String,
    app_version: String,
    platform: String,
    report: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Queued {
    endpoint: String,
    payload: Report,
}

struct State {
    settings: Settings,
    pending: BTreeMap<String, Queued>,
    last_sent_at_ms: Option<u64>,
    last_report_id: Option<String>,
    error: Option<String>,
}

pub struct Inbox {
    directory: PathBuf,
    state: Mutex<State>,
    sending: Mutex<()>,
    wake: Notify,
    client: reqwest::Client,
}

fn endpoint(raw: &str) -> Result<String> {
    let url = reqwest::Url::parse(raw.trim()).context("Enter a valid inbox HTTPS address")?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && loopback),
        "The inbox address must use HTTPS (HTTP is allowed only on this computer)"
    );
    ensure!(
        url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && matches!(url.path(), "" | "/"),
        "Use the inbox origin only, without a path, credentials, query, or fragment"
    );
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

fn label(raw: &str, name: &str) -> Result<String> {
    let value = raw.trim();
    ensure!(
        value.chars().count() <= 80 && !value.chars().any(char::is_control),
        "{name} must contain at most 80 characters without control characters"
    );
    Ok(value.to_owned())
}

fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn validate_input(input: &ReportInput) -> Result<()> {
    ensure!(
        matches!(
            input.suite.as_str(),
            "all" | "device" | "network" | "media" | "wallet" | "checklist"
        ),
        "Unknown diagnostic suite"
    );
    ensure!(
        (1..=9_007_199_254_740_991).contains(&input.generated_at_ms),
        "The report needs a generation time within the supported timestamp range"
    );
    ensure!(
        !input.report.trim().is_empty() && input.report.len() <= MAX_REPORT_BYTES,
        "Diagnostic reports must contain between 1 byte and 240 KiB"
    );
    Ok(())
}

fn private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "Invalid diagnostic inbox directory"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", hex::encode(rand::random::<[u8; 8]>())));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        fs::File::open(path.parent().context("Missing inbox directory")?)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

impl Inbox {
    pub fn open(root: &Path) -> Result<Arc<Self>> {
        let directory = root.join("diagnostic-inbox");
        private_directory(&directory).context("Cannot create private diagnostic inbox storage")?;
        private_directory(&directory.join("queue"))?;
        let mut error = None;
        let settings = match fs::read(directory.join("settings.json")) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(saved) => {
                    let input = Configure {
                        enabled: saved.enabled,
                        endpoint: saved.endpoint.clone(),
                        upload_token: Some(saved.upload_token.clone()),
                        device: saved.device.clone(),
                        session: saved.session.clone(),
                    };
                    match Self::settings(&Settings::default(), input) {
                        Ok(settings) => settings,
                        Err(_) => {
                            error = Some("Saved inbox settings are invalid. Uploads are paused; configure the inbox again.".into());
                            Settings::default()
                        }
                    }
                }
                Err(_) => {
                    error = Some("Saved inbox settings could not be read. Uploads are paused; configure the inbox again.".into());
                    Settings::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(_) => {
                error = Some("Saved inbox settings could not be read. Uploads are paused.".into());
                Settings::default()
            }
        };
        let mut pending = BTreeMap::new();
        for entry in fs::read_dir(directory.join("queue"))? {
            let entry = entry?;
            if entry.path().extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let read = (|| -> Result<Queued> {
                ensure!(entry.file_type()?.is_file(), "Invalid queued report");
                ensure!(
                    entry.metadata()?.len() <= (MAX_PAYLOAD_BYTES + 4096) as u64,
                    "Invalid queued report size"
                );
                let queued: Queued = serde_json::from_slice(&fs::read(entry.path())?)?;
                ensure!(
                    valid_id(&queued.payload.id)
                        && entry
                            .path()
                            .file_stem()
                            .is_some_and(|name| name == queued.payload.id.as_str()),
                    "Invalid queued report ID"
                );
                ensure!(
                    endpoint(&queued.endpoint)? == queued.endpoint,
                    "Invalid queued destination"
                );
                validate_input(&ReportInput {
                    suite: queued.payload.suite.clone(),
                    generated_at_ms: queued.payload.generated_at_ms,
                    report: queued.payload.report.clone(),
                })?;
                ensure!(
                    queued.payload.schema_version == 1
                        && serde_json::to_vec(&queued.payload)?.len() <= MAX_PAYLOAD_BYTES,
                    "Invalid queued payload"
                );
                ensure!(pending.len() < MAX_PENDING, "Too many queued reports");
                Ok(queued)
            })();
            match read {
                Ok(queued) => {
                    pending.insert(queued.payload.id.clone(), queued);
                }
                Err(_) => {
                    error = Some(
                        "A queued report could not be read; it remains on disk for recovery."
                            .into(),
                    )
                }
            }
        }
        Ok(Arc::new(Self {
            directory,
            state: Mutex::new(State {
                settings,
                pending,
                last_sent_at_ms: None,
                last_report_id: None,
                error,
            }),
            sending: Mutex::new(()),
            wake: Notify::new(),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!(
                    "ClipClub/",
                    env!("CARGO_PKG_VERSION"),
                    " (diagnostic inbox client)"
                ))
                .timeout(Duration::from_secs(15))
                .connect_timeout(Duration::from_secs(5))
                .build()?,
        }))
    }

    fn settings(previous: &Settings, input: Configure) -> Result<Settings> {
        let endpoint = if input.endpoint.trim().is_empty() {
            String::new()
        } else {
            endpoint(&input.endpoint)?
        };
        // A saved credential belongs to its origin. Leaving the key blank may
        // retain it only while editing that same inbox, never for a new host.
        let explicit_token = input.upload_token.filter(|token| !token.is_empty());
        let changed_origin = endpoint != previous.endpoint;
        ensure!(
            !input.enabled || !changed_origin || explicit_token.is_some(),
            "Enter a fresh upload token when changing the inbox address"
        );
        let upload_token = explicit_token.unwrap_or_else(|| {
            if changed_origin {
                String::new()
            } else {
                previous.upload_token.clone()
            }
        });
        ensure!(
            upload_token.is_empty()
                || ((32..=512).contains(&upload_token.len())
                    && upload_token.bytes().all(|b| b.is_ascii_graphic())),
            "Use a valid inbox upload token (32–512 characters without spaces)"
        );
        let device = label(&input.device, "Device name")?;
        let session = label(&input.session, "Test session")?;
        ensure!(
            !input.enabled
                || (!endpoint.is_empty() && !upload_token.is_empty() && !device.is_empty()),
            "Enter the inbox address, upload token, and device name before enabling uploads"
        );
        Ok(Settings {
            enabled: input.enabled,
            endpoint,
            upload_token,
            device,
            session,
        })
    }

    fn snapshot(state: &State) -> Value {
        json!({
            "enabled":state.settings.enabled,
            "endpoint":state.settings.endpoint,
            "configured":!state.settings.endpoint.is_empty() && !state.settings.upload_token.is_empty() && !state.settings.device.is_empty(),
            "device":state.settings.device,
            "session":state.settings.session,
            "pending":state.pending.len(),
            "last_sent_at_ms":state.last_sent_at_ms,
            "last_report_id":state.last_report_id,
            "error":state.error,
        })
    }

    pub async fn status(&self) -> Value {
        Self::snapshot(&*self.state.lock().await)
    }

    pub async fn configure(&self, input: Configure) -> Result<Value> {
        let mut state = self.state.lock().await;
        let settings = Self::settings(&state.settings, input)?;
        write_atomic(
            &self.directory.join("settings.json"),
            &serde_json::to_vec(&settings)?,
        )
        .context("Could not save inbox settings")?;
        state.settings = settings;
        state.error = None;
        self.wake.notify_one();
        Ok(Self::snapshot(&state))
    }

    pub async fn enqueue(&self, input: ReportInput) -> Result<Value> {
        validate_input(&input)?;
        let mut state = self.state.lock().await;
        ensure!(
            state.settings.enabled,
            "Enable automatic diagnostic uploads before sending a report"
        );
        ensure!(
            state.pending.len() < MAX_PENDING,
            "The inbox queue has 100 reports. Retry delivery before adding more"
        );
        let id = hex::encode(rand::random::<[u8; 16]>());
        let queued = Queued {
            endpoint: state.settings.endpoint.clone(),
            payload: Report {
                schema_version: 1,
                id: id.clone(),
                generated_at_ms: input.generated_at_ms,
                device: state.settings.device.clone(),
                session: state.settings.session.clone(),
                suite: input.suite,
                app_version: env!("CARGO_PKG_VERSION").into(),
                platform: format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH),
                report: input.report,
            },
        };
        ensure!(
            serde_json::to_vec(&queued.payload)?.len() <= MAX_PAYLOAD_BYTES,
            "The encoded diagnostic report exceeds 256 KiB"
        );
        write_atomic(
            &self.directory.join("queue").join(format!("{id}.json")),
            &serde_json::to_vec(&queued)?,
        )
        .context("Could not save the report for delivery")?;
        state.pending.insert(id.clone(), queued);
        state.error = None;
        self.wake.notify_one();
        let mut status = Self::snapshot(&state);
        status["id"] = id.into();
        Ok(status)
    }

    pub async fn retry(&self) -> Value {
        self.wake.notify_one();
        self.status().await
    }

    pub fn start(self: &Arc<Self>, shutdown: CancellationToken) {
        let inbox = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => (),
                    _ = inbox.wake.notified() => (),
                }
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    _ = inbox.deliver() => (),
                }
            }
        });
    }

    async fn upload(&self, queued: &Queued, token: &str) -> Result<()> {
        let response = self
            .client
            .put(format!(
                "{}/v1/reports/{}",
                queued.endpoint, queued.payload.id
            ))
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&queued.payload)?)
            .send()
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "The diagnostic inbox could not be reached. Reports remain queued for retry."
                )
            })?;
        ensure!(
            response.status().is_success(),
            "The inbox did not accept the report (HTTP {}). Check the upload token and inbox address; the report remains queued.",
            response.status().as_u16()
        );
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            anyhow::anyhow!("The inbox acknowledgement was interrupted; the report remains queued.")
        })? {
            ensure!(
                bytes.len() + chunk.len() <= 1024,
                "The inbox returned an invalid acknowledgement; the report remains queued"
            );
            bytes.extend_from_slice(&chunk);
        }
        let ack: Value = serde_json::from_slice(&bytes)
            .context("The inbox returned an invalid acknowledgement; the report remains queued")?;
        ensure!(
            ack["id"].as_str() == Some(&queued.payload.id),
            "The inbox acknowledged a different report; the report remains queued"
        );
        Ok(())
    }

    pub async fn deliver(&self) {
        let Ok(_sending) = self.sending.try_lock() else {
            return;
        };
        loop {
            let next = {
                let state = self.state.lock().await;
                if !state.settings.enabled {
                    return;
                }
                state
                    .pending
                    .values()
                    .find(|queued| queued.endpoint == state.settings.endpoint)
                    .map(|queued| (queued.clone(), state.settings.upload_token.clone()))
            };
            let Some((queued, token)) = next else {
                return;
            };
            let result = self.upload(&queued, &token).await;
            let mut state = self.state.lock().await;
            match result {
                Ok(()) => {
                    if fs::remove_file(
                        self.directory
                            .join("queue")
                            .join(format!("{}.json", queued.payload.id)),
                    )
                    .is_err()
                    {
                        state.error = Some("The inbox received the report, but its local queue entry could not be removed. Delivery will be retried safely.".into());
                        return;
                    }
                    state.pending.remove(&queued.payload.id);
                    state.last_sent_at_ms = Some(crate::core::current_time_ms());
                    state.last_report_id = Some(queued.payload.id);
                    state.error = None;
                }
                Err(error) => {
                    state.error = Some(error.to_string());
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        extract::{Path, State},
        http::{HeaderMap, StatusCode},
        routing::put,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn config(endpoint: &str, enabled: bool) -> Configure {
        Configure {
            enabled,
            endpoint: endpoint.into(),
            upload_token: Some("test-upload-secret-token-0123456789".into()),
            device: "Linux laptop".into(),
            session: "hotspot".into(),
        }
    }
    fn report() -> ReportInput {
        ReportInput {
            suite: "network".into(),
            generated_at_ms: 12345,
            report: "Peer reachability failed\n".into(),
        }
    }

    async fn server(
        redirect: Option<String>,
    ) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let state = (calls.clone(), redirect);
        let router = Router::new()
            .route(
                "/v1/reports/{id}",
                put(
                    async |Path(id): Path<String>,
                           State((calls, redirect)): State<(Arc<AtomicUsize>, Option<String>)>,
                           headers: HeaderMap,
                           body: String| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(
                            headers["authorization"],
                            "Bearer test-upload-secret-token-0123456789"
                        );
                        assert_eq!(
                            headers["user-agent"],
                            concat!(
                                "ClipClub/",
                                env!("CARGO_PKG_VERSION"),
                                " (diagnostic inbox client)"
                            )
                        );
                        let payload: Value = serde_json::from_str(&body).unwrap();
                        assert_eq!(payload["id"], id);
                        assert_eq!(payload["device"], "Linux laptop");
                        if let Some(location) = redirect {
                            return (
                                StatusCode::TEMPORARY_REDIRECT,
                                [("location", location)],
                                String::new(),
                            );
                        }
                        (
                            StatusCode::CREATED,
                            [("content-type", "application/json".into())],
                            json!({"id":id}).to_string(),
                        )
                    },
                ),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (address, calls, task)
    }

    #[tokio::test]
    async fn disabled_by_default_and_secrets_are_not_returned() {
        let temp = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(temp.path()).unwrap();
        assert_eq!(inbox.status().await["enabled"], false);
        assert!(inbox.enqueue(report()).await.is_err());
        let status = inbox
            .configure(config("https://inbox.example/", true))
            .await
            .unwrap();
        assert!(!status.to_string().contains("test-upload-secret-token"));
        assert_eq!(status["endpoint"], "https://inbox.example");
        let mut preserve = config("https://inbox.example", false);
        preserve.upload_token = None;
        inbox.configure(preserve).await.unwrap();
        assert!(
            inbox
                .state
                .lock()
                .await
                .settings
                .upload_token
                .starts_with("test-upload")
        );
        assert!(inbox.enqueue(report()).await.is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(temp.path().join("diagnostic-inbox/settings.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn durable_retry_survives_offline_and_restart_then_uses_authentication() {
        let temp = tempfile::tempdir().unwrap();
        let (address, calls, task) = server(None).await;
        let inbox = Inbox::open(temp.path()).unwrap();
        inbox.configure(config(&address, true)).await.unwrap();
        let result = inbox.enqueue(report()).await.unwrap();
        let id = result["id"].as_str().unwrap().to_owned();
        drop(inbox);
        let inbox = Inbox::open(temp.path()).unwrap();
        assert_eq!(inbox.status().await["pending"], 1);
        inbox.deliver().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(inbox.status().await["pending"], 0);
        assert_eq!(inbox.status().await["last_report_id"], id);
        task.abort();
        inbox.enqueue(report()).await.unwrap();
        inbox.deliver().await;
        assert_eq!(inbox.status().await["pending"], 1);
        let status = inbox.status().await;
        assert!(!status.to_string().contains("test-upload-secret-token"));
        drop(inbox);
        let inbox = Inbox::open(temp.path()).unwrap();
        assert_eq!(inbox.status().await["pending"], 1);
    }

    #[tokio::test]
    async fn redirects_are_not_followed_and_destination_changes_do_not_leak_old_reports() {
        let temp = tempfile::tempdir().unwrap();
        let (target, target_calls, target_task) = server(None).await;
        let (address, calls, task) = server(Some(format!("{target}/v1/reports/redirected"))).await;
        let inbox = Inbox::open(temp.path()).unwrap();
        inbox.configure(config(&address, true)).await.unwrap();
        inbox.enqueue(report()).await.unwrap();
        inbox.deliver().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(target_calls.load(Ordering::SeqCst), 0);
        inbox.configure(config(&target, true)).await.unwrap();
        inbox.deliver().await;
        assert_eq!(target_calls.load(Ordering::SeqCst), 0);
        assert_eq!(inbox.status().await["pending"], 1);
        inbox.enqueue(report()).await.unwrap();
        inbox.deliver().await;
        assert_eq!(target_calls.load(Ordering::SeqCst), 1);
        assert_eq!(inbox.status().await["pending"], 1);
        target_task.abort();
        task.abort();
    }

    #[tokio::test]
    async fn queue_and_payload_are_bounded_without_dropping_existing_reports() {
        let temp = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(temp.path()).unwrap();
        inbox
            .configure(config("https://inbox.example", true))
            .await
            .unwrap();
        let mut invalid_time = report();
        invalid_time.generated_at_ms = 9_007_199_254_740_992;
        assert!(inbox.enqueue(invalid_time).await.is_err());
        let mut oversized = report();
        oversized.report = "x".repeat(MAX_REPORT_BYTES + 1);
        assert!(inbox.enqueue(oversized).await.is_err());
        let mut expanded = report();
        expanded.report = "\u{0001}".repeat(MAX_REPORT_BYTES / 2);
        assert!(inbox.enqueue(expanded).await.is_err());
        for _ in 0..MAX_PENDING {
            inbox.enqueue(report()).await.unwrap();
        }
        assert!(inbox.enqueue(report()).await.is_err());
        assert_eq!(inbox.status().await["pending"], MAX_PENDING);
        assert_eq!(
            Inbox::open(temp.path()).unwrap().status().await["pending"],
            MAX_PENDING
        );
    }

    #[tokio::test]
    async fn disabling_pauses_the_durable_queue_until_reenabled() {
        let temp = tempfile::tempdir().unwrap();
        let (address, calls, task) = server(None).await;
        let inbox = Inbox::open(temp.path()).unwrap();
        inbox.configure(config(&address, true)).await.unwrap();
        inbox.enqueue(report()).await.unwrap();
        inbox.configure(config(&address, false)).await.unwrap();
        inbox.deliver().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(inbox.status().await["pending"], 1);
        drop(inbox);
        let inbox = Inbox::open(temp.path()).unwrap();
        inbox.deliver().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        inbox.configure(config(&address, true)).await.unwrap();
        inbox.deliver().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(inbox.status().await["pending"], 0);
        task.abort();
    }

    #[test]
    fn short_tokens_are_rejected_without_echoing_them() {
        let mut input = config("https://inbox.example", true);
        input.upload_token = Some("too-short-secret".into());
        let result = Inbox::settings(&Settings::default(), input);
        assert!(result.is_err());
        assert!(
            !result
                .err()
                .unwrap()
                .to_string()
                .contains("too-short-secret")
        );
    }

    #[tokio::test]
    async fn blank_tokens_are_retained_only_for_the_same_inbox_origin() {
        let temp = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(temp.path()).unwrap();
        inbox
            .configure(config("https://first.example", true))
            .await
            .unwrap();
        let mut same = config("https://first.example:443/", true);
        same.upload_token = None;
        assert!(inbox.configure(same).await.is_ok());
        assert!(!inbox.state.lock().await.settings.upload_token.is_empty());
        for token in [None, Some(String::new())] {
            let mut changed = config("https://second.example", true);
            changed.upload_token = token;
            let error = inbox.configure(changed).await.unwrap_err();
            assert!(error.to_string().contains("fresh upload token"));
            assert_eq!(inbox.status().await["endpoint"], "https://first.example");
        }
        let mut disabled = config("https://second.example", false);
        disabled.upload_token = None;
        let status = inbox.configure(disabled).await.unwrap();
        assert_eq!(status["configured"], false);
        assert!(inbox.state.lock().await.settings.upload_token.is_empty());
        drop(inbox);
        let inbox = Inbox::open(temp.path()).unwrap();
        assert!(inbox.state.lock().await.settings.upload_token.is_empty());
        let mut enable = config("https://second.example", true);
        enable.upload_token = None;
        assert!(inbox.configure(enable).await.is_err());
        let mut fresh = config("https://second.example", true);
        fresh.upload_token = Some("fresh-token-for-second-inbox-0123456789".into());
        assert!(inbox.configure(fresh).await.is_ok());
    }

    #[test]
    fn endpoints_exclude_credentials_and_unencrypted_remote_hosts() {
        for url in [
            "http://example.com",
            "https://name:secret@example.com",
            "https://example.com/path",
            "https://example.com?secret",
            "https://example.com/#fragment",
        ] {
            assert!(endpoint(url).is_err(), "accepted {url}");
        }
        assert_eq!(
            endpoint("https://example.com/").unwrap(),
            "https://example.com"
        );
        assert!(endpoint("http://127.0.0.1:1234").is_ok());
        assert!(endpoint("http://[::1]:1234").is_ok());
    }
}
