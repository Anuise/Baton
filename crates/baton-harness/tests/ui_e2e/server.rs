//! 測試專用的「桌面殼」：在 loopback 上提供 `web/dist`，並把 `/api/` 的請求轉給 Harness 的 Runtime。
//!
//! 真實的桌面殼以 Tauri 指令帶憑證呼叫 Runtime；這裡改以 HTTP 轉送，讓瀏覽器中的 Web UI（`createHttpClient`）
//! 走真實的 HTTP API。轉送時與殼相同：帶上桌面憑證與固定標頭，不帶瀏覽器的 Origin、Cookie 等標頭。
//! Runtime 的 API 防護完全不放寬。

use std::path::{Component, Path, PathBuf};

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use baton_api::{CLIENT_HEADER, CLIENT_HEADER_VALUE};
use baton_harness::Harness;

/// 轉送時保留的回應標頭（成果下載的雜湊值以標頭回傳）。
const FORWARDED_RESPONSE_HEADERS: [&str; 2] = ["content-type", "x-baton-sha256"];

#[derive(Clone)]
struct Shell {
    dist: PathBuf,
    runtime_url: String,
    credential: String,
    http: reqwest::Client,
}

pub struct WebUiServer {
    addr: std::net::SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl WebUiServer {
    /// 以目前 Runtime 的位址與桌面憑證啟動（E2E 不重啟 Runtime；重啟後要重新啟動伺服器）。
    pub async fn start(harness: &Harness, dist: PathBuf) -> Self {
        let shell = Shell {
            dist,
            runtime_url: harness.runtime_url(),
            credential: harness.desktop_credential(),
            http: reqwest::Client::builder().no_proxy().build().expect("無法建立 HTTP 用戶端"),
        };
        let app = axum::Router::new().fallback(serve).with_state(shell);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("無法監聽 loopback");
        let addr = listener.local_addr().expect("取不到監聽位址");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("Web UI 伺服器中止");
        });
        Self { addr, task }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

impl Drop for WebUiServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(State(shell): State<Shell>, request: Request) -> Response {
    if request.uri().path().starts_with("/api/") {
        forward(&shell, request).await
    } else {
        static_file(&shell.dist, request.uri().path()).await
    }
}

async fn forward(shell: &Shell, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let path_and_query = parts.uri.path_and_query().map_or("/", |p| p.as_str());
    let body = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(body) => body,
        Err(error) => return (StatusCode::BAD_REQUEST, format!("無法讀取請求本文：{error}")).into_response(),
    };
    let mut outgoing = shell
        .http
        .request(parts.method, format!("{}{path_and_query}", shell.runtime_url))
        .bearer_auth(&shell.credential)
        .header(CLIENT_HEADER, CLIENT_HEADER_VALUE);
    if let Some(content_type) = parts.headers.get(header::CONTENT_TYPE) {
        outgoing = outgoing.header(header::CONTENT_TYPE, content_type);
    }
    if !body.is_empty() {
        outgoing = outgoing.body(body);
    }
    let response = match outgoing.send().await {
        Ok(response) => response,
        Err(error) => return (StatusCode::BAD_GATEWAY, format!("無法連到 Runtime：{error}")).into_response(),
    };
    let mut forwarded = Response::builder().status(response.status().as_u16());
    for name in FORWARDED_RESPONSE_HEADERS {
        if let Some(value) = response.headers().get(name) {
            forwarded = forwarded.header(name, value.as_bytes());
        }
    }
    let bytes = response.bytes().await.unwrap_or_default();
    forwarded.body(Body::from(bytes)).expect("回應標頭來自 Runtime，一定合法")
}

/// 提供 `dist` 中的檔案；不是檔案的路徑一律回 `index.html`（單頁 App）。
async fn static_file(dist: &Path, path: &str) -> Response {
    let relative = Path::new(path.trim_start_matches('/'));
    // 只接受一般的路徑元件，不讓 `..` 跳出 dist
    let safe = relative.components().all(|c| matches!(c, Component::Normal(_)));
    let candidate = dist.join(relative);
    let file = if safe && candidate.is_file() { candidate } else { dist.join("index.html") };
    match tokio::fs::read(&file).await {
        Ok(bytes) => {
            let mut response = Bytes::from(bytes).into_response();
            response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(&file)));
            response
        }
        Err(error) => (StatusCode::NOT_FOUND, format!("無法讀取 {}：{error}", file.display())).into_response(),
    }
}

fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}
