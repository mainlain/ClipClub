use crate::{
    app::App,
    core::{MAX_FILE_BYTES, Settings},
};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Clone)]
struct HttpState {
    app: Arc<App>,
    token: String,
    host: String,
}
type ApiResult = Result<Json<Value>, ApiError>;
struct ApiError(anyhow::Error);
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(error: E) -> Self {
        Self(error.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":self.0.to_string()})),
        )
            .into_response()
    }
}

pub async fn serve(
    app: Arc<App>,
    port: u16,
) -> anyhow::Result<(String, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let host = listener.local_addr()?.to_string();
    let token = hex::encode(rand::random::<[u8; 32]>());
    let state = HttpState {
        app: app.clone(),
        token,
        host: host.clone(),
    };
    let router = Router::new()
        .route("/", get(index))
        .route("/app.js", get(js))
        .route("/style.css", get(css))
        .route("/api/state", get(status))
        .route("/api/upload", post(upload))
        .route("/api/rate", post(rate))
        .route("/api/roll", post(roll))
        .route("/api/session", post(session))
        .route("/api/settings", post(settings))
        .route("/api/remove/{id}", post(remove))
        .route("/api/video/{id}", get(video))
        .route("/api/torrent/{id}", get(metainfo))
        .layer(DefaultBodyLimit::max((MAX_FILE_BYTES + 10_000) as usize))
        .layer(middleware::from_fn_with_state(state.clone(), security))
        .with_state(state);
    let cancel = app.shutdown.clone();
    let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, router)
            .with_graceful_shutdown(cancel.cancelled_owned())
            .await
        {
            eprintln!("Local interface: {error}");
        }
    });
    Ok((format!("http://{host}"), task))
}
async fn security(State(state): State<HttpState>, request: Request, next: Next) -> Response {
    // Prevent DNS rebinding and cross-origin sites from controlling the local client.
    if request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        != Some(state.host.as_str())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(origin) = request.headers().get(header::ORIGIN)
        && origin.to_str().ok() != Some(format!("http://{}", state.host).as_str())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if request.uri().path().starts_with("/api/") {
        let header_token = request
            .headers()
            .get("x-clipcrop-token")
            .and_then(|v| v.to_str().ok());
        let query_token = request
            .uri()
            .query()
            .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("token=")));
        if header_token != Some(&state.token) && query_token != Some(&state.token) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"Invalid local session"})),
            )
                .into_response();
        }
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, "default-src 'self'; script-src 'self'; style-src 'self'; media-src 'self' blob:; img-src 'self' data:; connect-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'".parse().unwrap());
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    headers.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
async fn index(State(state): State<HttpState>) -> Html<String> {
    Html(include_str!("../ui/index.html").replace("__TOKEN__", &state.token))
}
async fn js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../ui/app.js"),
    )
}
async fn css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../ui/style.css"),
    )
}
async fn status(State(state): State<HttpState>) -> Json<Value> {
    Json(state.app.state().await)
}
async fn upload(State(state): State<HttpState>, mut multipart: Multipart) -> ApiResult {
    let field = multipart
        .next_field()
        .await?
        .ok_or_else(|| anyhow::anyhow!("Choose a video file"))?;
    let name = field
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Missing filename"))?
        .to_owned();
    let bytes = field.bytes().await?;
    if multipart.next_field().await?.is_some() {
        return Err(anyhow::anyhow!("Upload one video at a time").into());
    }
    Ok(Json(state.app.upload(name, bytes.to_vec()).await?))
}
#[derive(Deserialize)]
struct Rate {
    id: String,
    score: u8,
}
async fn rate(State(state): State<HttpState>, Json(input): Json<Rate>) -> ApiResult {
    Ok(Json(state.app.rate(&input.id, input.score).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Roll {
    wager: f64,
}
async fn roll(State(state): State<HttpState>, Json(input): Json<Roll>) -> ApiResult {
    Ok(Json(state.app.roll(input.wager).await?))
}
#[derive(Deserialize)]
struct Session {
    active: bool,
}
async fn session(State(state): State<HttpState>, Json(input): Json<Session>) -> ApiResult {
    state.app.session(input.active).await?;
    Ok(Json(json!({"ok":true})))
}
async fn settings(State(state): State<HttpState>, Json(input): Json<Settings>) -> ApiResult {
    state.app.settings(input).await?;
    Ok(Json(json!({"ok":true})))
}
async fn remove(State(state): State<HttpState>, Path(id): Path<String>) -> ApiResult {
    state.app.remove(&id).await?;
    Ok(Json(json!({"ok":true})))
}
async fn metainfo(
    State(state): State<HttpState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let store = state.app.store.lock().await;
    let clip = store
        .data
        .catalog
        .get(&id)
        .ok_or_else(|| anyhow::anyhow!("Unknown clip"))?;
    let torrent = crate::app::prepared(clip)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/x-bittorrent".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}.torrent\"", clip.id),
            ),
        ],
        torrent.bytes,
    )
        .into_response())
}
async fn video(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (path, extension) = state.app.media_path(&id).await?;
    let meta = tokio::fs::symlink_metadata(&path).await?;
    if !meta.file_type().is_file() || meta.len() == 0 || meta.len() > MAX_FILE_BYTES {
        return Err(anyhow::anyhow!("Invalid managed video").into());
    }
    let size = meta.len();
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let (start, end) = match range {
        Some(value) => match parse_range(value, size) {
            Some(range) => range,
            None => {
                return Ok((
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [(header::CONTENT_RANGE, format!("bytes */{size}"))],
                )
                    .into_response());
            }
        },
        None => (0, size - 1),
    };
    let mut file = tokio::fs::File::open(path).await?;
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut bytes = vec![0; (end - start + 1) as usize];
    file.read_exact(&mut bytes).await?;
    let mut response = Response::builder()
        .status(if range.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(
            header::CONTENT_TYPE,
            if extension == "webm" {
                "video/webm"
            } else {
                "video/mp4"
            },
        )
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, bytes.len());
    if range.is_some() {
        response = response.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    Ok(response.body(Body::from(bytes)).unwrap())
}
fn parse_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let range = value.strip_prefix("bytes=")?;
    if range.contains(',') || size == 0 {
        return None;
    }
    let (a, b) = range.split_once('-')?;
    if a.is_empty() {
        let suffix = b.parse::<u64>().ok()?;
        return (suffix > 0).then_some((size.saturating_sub(suffix), size - 1));
    }
    let start = a.parse::<u64>().ok()?;
    let end = if b.is_empty() {
        size - 1
    } else {
        b.parse::<u64>().ok()?.min(size - 1)
    };
    (start < size && start <= end).then_some((start, end))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_video_ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(parse_range("bytes=95-", 100), Some((95, 99)));
        assert_eq!(parse_range("bytes=-5", 100), Some((95, 99)));
        assert_eq!(parse_range("bytes=0-999", 100), Some((0, 99)));
        for range in [
            "bytes=100-",
            "bytes=5-2",
            "bytes=-0",
            "bytes=0-1,4-5",
            "nonsense",
        ] {
            assert_eq!(parse_range(range, 100), None);
        }
    }
}
