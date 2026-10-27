//! 正式版的 Anthropic models API 查詢（AI-33）：使用者貼上 API 金鑰更新模型設定的可用清單。
//!
//! 金鑰只放在這一次請求的 `x-api-key` 標頭，只送往 `api.anthropic.com`；不保存、不記錄，錯誤訊息不含它。
//! 不跟隨轉址，避免標頭被帶到其他主機。

use std::time::Duration;

use async_trait::async_trait;
use baton_ports::{ModelDirectory, ModelDirectoryError, ModelPage};

const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// 每頁筆數用 API 的上限，通常一頁就取完。
const PAGE_LIMIT: &str = "1000";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub struct AnthropicModelDirectory {
    base_url: String,
    http: reqwest::Client,
}

impl Default for AnthropicModelDirectory {
    fn default() -> Self {
        Self::with_base_url(ANTHROPIC_API_BASE)
    }
}

impl AnthropicModelDirectory {
    /// 只有測試能改連線位置（本機模擬伺服器）；正式版一律是 `api.anthropic.com`。
    fn with_base_url(base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self { base_url: base_url.trim_end_matches('/').to_string(), http }
    }
}

#[async_trait]
impl ModelDirectory for AnthropicModelDirectory {
    async fn page(&self, api_key: &str, after_id: Option<&str>) -> Result<ModelPage, ModelDirectoryError> {
        let mut query = vec![("limit", PAGE_LIMIT)];
        if let Some(after_id) = after_id {
            query.push(("after_id", after_id));
        }
        let response = self
            .http
            .get(format!("{}/v1/models", self.base_url))
            .query(&query)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .send()
            .await
            // reqwest 的錯誤只帶網址（不含標頭），金鑰不會出現在訊息中
            .map_err(|e| ModelDirectoryError::Unreachable(e.without_url().to_string()))?;
        let status = response.status().as_u16();
        match status {
            200..=299 => {}
            401 | 403 => return Err(ModelDirectoryError::KeyRejected(status)),
            _ => return Err(ModelDirectoryError::Status(status)),
        }
        let body = response.bytes().await.map_err(|e| ModelDirectoryError::Unreachable(e.without_url().to_string()))?;
        serde_json::from_slice(&body).map_err(|e| ModelDirectoryError::Invalid(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::extract::{Query, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::Router;
    use baton_ports::RemoteModel;

    use super::*;

    type Seen = Arc<Mutex<Vec<(HeaderMap, Vec<(String, String)>)>>>;

    /// 本機模擬的 models API：記下每個請求的標頭與查詢參數，回應固定的狀態與內容。
    async fn mock(status: StatusCode, body: &'static str) -> (String, Seen) {
        let seen: Seen = Arc::default();
        let app = Router::new()
            .route(
                "/v1/models",
                get(move |State(seen): State<Seen>, headers: HeaderMap, Query(query): Query<Vec<(String, String)>>| async move {
                    seen.lock().unwrap().push((headers, query));
                    (status, body).into_response()
                }),
            )
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), seen)
    }

    #[test]
    fn production_always_talks_to_api_anthropic_com() {
        assert_eq!(AnthropicModelDirectory::default().base_url, "https://api.anthropic.com");
    }

    #[tokio::test]
    async fn sends_the_key_and_version_headers_and_the_after_id_and_parses_a_page() {
        let (base, seen) = mock(
            StatusCode::OK,
            r#"{"data":[{"type":"model","id":"claude-opus-5-5","display_name":"Claude Opus 5.5","created_at":"2026-01-01T00:00:00Z"}],
                "has_more":true,"first_id":"claude-opus-5-5","last_id":"claude-opus-5-5"}"#,
        )
        .await;
        let directory = AnthropicModelDirectory::with_base_url(&base);

        let page = directory.page("sk-ant-test", Some("claude-fable-5-1")).await.unwrap();

        assert_eq!(
            page,
            ModelPage {
                data: vec![RemoteModel { id: "claude-opus-5-5".into(), display_name: "Claude Opus 5.5".into() }],
                has_more: true,
                last_id: Some("claude-opus-5-5".into()),
            }
        );
        let seen = seen.lock().unwrap();
        let (headers, query) = &seen[0];
        assert_eq!(headers["x-api-key"], "sk-ant-test");
        assert_eq!(headers["anthropic-version"], "2023-06-01");
        assert!(query.contains(&("after_id".into(), "claude-fable-5-1".into())), "{query:?}");
        assert!(query.contains(&("limit".into(), "1000".into())), "{query:?}");
    }

    #[tokio::test]
    async fn the_first_page_has_no_after_id() {
        let (base, seen) = mock(StatusCode::OK, r#"{"data":[],"has_more":false,"last_id":null}"#).await;

        let page = AnthropicModelDirectory::with_base_url(&base).page("sk-ant-test", None).await.unwrap();

        assert_eq!(page, ModelPage { data: vec![], has_more: false, last_id: None });
        assert!(!seen.lock().unwrap()[0].1.iter().any(|(name, _)| name == "after_id"));
    }

    #[tokio::test]
    async fn unauthorized_is_a_rejected_key_and_other_statuses_are_reported_without_the_key() {
        let (base, _) = mock(StatusCode::UNAUTHORIZED, r#"{"type":"error","error":{"type":"authentication_error"}}"#).await;
        let error = AnthropicModelDirectory::with_base_url(&base).page("sk-ant-secret", None).await.unwrap_err();
        assert_eq!(error, ModelDirectoryError::KeyRejected(401));

        let (base, _) = mock(StatusCode::INTERNAL_SERVER_ERROR, "boom").await;
        let error = AnthropicModelDirectory::with_base_url(&base).page("sk-ant-secret", None).await.unwrap_err();
        assert_eq!(error, ModelDirectoryError::Status(500));

        let (base, _) = mock(StatusCode::OK, "not json").await;
        let error = AnthropicModelDirectory::with_base_url(&base).page("sk-ant-secret", None).await.unwrap_err();
        assert!(matches!(&error, ModelDirectoryError::Invalid(_)), "{error:?}");
        assert!(!error.to_string().contains("sk-ant-secret"));
    }

    #[tokio::test]
    async fn an_unreachable_server_is_a_network_error_without_the_key() {
        // 先綁定再放掉，取得一個沒有人監聽的埠
        let port = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
        let directory = AnthropicModelDirectory::with_base_url(&format!("http://127.0.0.1:{port}"));

        let error = directory.page("sk-ant-secret", None).await.unwrap_err();

        assert!(matches!(&error, ModelDirectoryError::Unreachable(_)), "{error:?}");
        assert!(!error.to_string().contains("sk-ant-secret"));
    }
}
