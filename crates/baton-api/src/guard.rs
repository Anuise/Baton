//! 本機 API 的入口防護（spec D-22、D-23）：套用在所有端點（含串流與未知路徑）。
//!
//! 檢查順序：Host → CORS 預檢 → Origin → 固定自訂標頭 → bearer 憑證 → 角色。
//! 任何一關失敗都直接回應錯誤，不進入路由；也不會回應任何 CORS 標頭，
//! 所以瀏覽器中的其他網頁既送不出預檢、也讀不到回應。

use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use axum::extract::{MatchedPath, Request, State};
use axum::http::header::{AUTHORIZATION, HOST, ORIGIN};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use baton_domain::agent::{CONTROLLER_TOOLS, TOOL_PERMISSION_PROMPT};
use baton_domain::auth::{hash_token, CredentialRecord, Role};

use crate::{ApiState, ErrorBody};

/// 每個請求都必須帶的自訂標頭。跨網站請求帶自訂標頭會觸發 CORS 預檢，而預檢一律被拒絕。
pub const CLIENT_HEADER: &str = "x-baton-client";
pub const CLIENT_HEADER_VALUE: &str = "baton-desktop";

/// Agent API 的路徑前綴：這個前綴之下只接受 Agent 角色，其餘路徑只接受桌面角色。
const AGENT_API_PREFIX: &str = "/agent/v1/";
/// Agent 工具的路徑前綴（後面接工具名稱）。
const AGENT_TOOLS_PATH: &str = "/agent/v1/tools/";

/// Tauri WebView 的 origin（Windows 預設為 http，啟用 https 方案時為 https；其他平台為 tauri 方案）。
const APP_ORIGINS: [&str; 3] = ["http://tauri.localhost", "https://tauri.localhost", "tauri://localhost"];

/// 請求被拒絕的原因；`code` 是穩定的機器可讀代碼，App 以它顯示失敗原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    HostNotAllowed,
    PreflightRefused,
    OriginNotAllowed,
    ClientHeaderMissing,
    CredentialMissing,
    CredentialInvalid,
    CredentialExpired,
    RoleNotAllowed,
    /// Worker 的憑證呼叫 Controller 專屬的工具（票 17）。
    ControllerOnly,
    /// Controller 的憑證呼叫 Worker 的工具（票 17）。
    WorkerOnly,
    Internal,
}

impl Denial {
    pub fn code(self) -> &'static str {
        match self {
            Denial::HostNotAllowed => "host_not_allowed",
            Denial::PreflightRefused => "preflight_refused",
            Denial::OriginNotAllowed => "origin_not_allowed",
            Denial::ClientHeaderMissing => "client_header_missing",
            Denial::CredentialMissing => "credential_missing",
            Denial::CredentialInvalid => "credential_invalid",
            Denial::CredentialExpired => "credential_expired",
            Denial::RoleNotAllowed => "role_not_allowed",
            Denial::ControllerOnly => "controller_only",
            Denial::WorkerOnly => "worker_only",
            Denial::Internal => "internal_error",
        }
    }

    fn status(self) -> StatusCode {
        match self {
            Denial::HostNotAllowed
            | Denial::OriginNotAllowed
            | Denial::ClientHeaderMissing
            | Denial::RoleNotAllowed
            | Denial::ControllerOnly
            | Denial::WorkerOnly => StatusCode::FORBIDDEN,
            Denial::PreflightRefused => StatusCode::METHOD_NOT_ALLOWED,
            Denial::CredentialMissing | Denial::CredentialInvalid | Denial::CredentialExpired => {
                StatusCode::UNAUTHORIZED
            }
            Denial::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// 給使用者看的說明；固定文字，不含請求內容，避免把憑證回顯出去。
    fn message(self) -> &'static str {
        match self {
            Denial::HostNotAllowed => "Host 不是本機 loopback 位址與 Runtime 埠",
            Denial::PreflightRefused => "不支援 CORS 預檢",
            Denial::OriginNotAllowed => "Origin 不是桌面 App",
            Denial::ClientHeaderMissing => "缺少或錯誤的 Baton 用戶端標頭",
            Denial::CredentialMissing => "請求沒有帶 Bearer 憑證",
            Denial::CredentialInvalid => "憑證無效（Runtime 可能已重新啟動，請重新讀取桌面憑證）",
            Denial::CredentialExpired => "憑證已過期",
            Denial::RoleNotAllowed => "這個憑證的角色不能使用這組 API",
            Denial::ControllerOnly => {
                "這個工具只有 Controller 能使用；Worker 需要調整工作時請以 report_blocked 或 request_decision 回報"
            }
            Denial::WorkerOnly => {
                "這個工具只有 Worker 能使用；Controller 請以 propose_task、dispatch_task 等工具把工作交給 Worker"
            }
            Denial::Internal => "Runtime 內部錯誤，無法驗證憑證",
        }
    }
}

impl IntoResponse for Denial {
    fn into_response(self) -> Response {
        let body = ErrorBody { error: self.code().to_string(), message: self.message().to_string() };
        (self.status(), Json(body)).into_response()
    }
}

/// 所有路由共用的防護；通過後才交給路由處理，並記錄請求結果（路徑只記路由樣板，不含實際 URL）。
pub(crate) async fn guard(State(state): State<Arc<ApiState>>, mut request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "(unmatched)".to_string(), |p| p.as_str().to_string());

    // 只借出標頭、方法與路徑：整個 Request 含有不可跨 await 共享的 Body
    let outcome = check(&state, request.method(), request.headers(), request.uri().path()).await;
    let reason = outcome.as_ref().err().map_or("ok", |denial| denial.code());
    let response = match outcome {
        Ok(record) => {
            // 讓 Agent API 的處理函式取得已驗證的憑證（含範圍）
            request.extensions_mut().insert(record);
            next.run(request).await
        }
        Err(denial) => denial.into_response(),
    };

    (state.log)(&format!("{method} {route} -> {} {reason}", response.status().as_u16()));
    response
}

async fn check(
    state: &ApiState,
    method: &Method,
    headers: &HeaderMap,
    path: &str,
) -> Result<CredentialRecord, Denial> {
    let host = headers.get(HOST).and_then(|v| v.to_str().ok()).unwrap_or_default();
    if !host_allowed(host, state.allowed_port) {
        return Err(Denial::HostNotAllowed);
    }
    if method == Method::OPTIONS {
        return Err(Denial::PreflightRefused);
    }
    if let Some(origin) = headers.get(ORIGIN) {
        if !origin.to_str().is_ok_and(origin_allowed) {
            return Err(Denial::OriginNotAllowed);
        }
    }
    if headers.get(CLIENT_HEADER).is_none_or(|v| v != CLIENT_HEADER_VALUE) {
        return Err(Denial::ClientHeaderMissing);
    }

    let token = bearer_token(headers).ok_or(Denial::CredentialMissing)?;
    let record = state
        .store
        .find_credential(&hash_token(token))
        .await
        .map_err(|_| Denial::Internal)?
        .ok_or(Denial::CredentialInvalid)?;
    if record.is_expired(state.clock.now()) {
        return Err(Denial::CredentialExpired);
    }
    // 管理 API 與 Agent API 分開路由、分開憑證：各自只接受自己的角色
    let required = if path.starts_with(AGENT_API_PREFIX) { Role::Agent } else { Role::Desktop };
    if record.role != required {
        return Err(Denial::RoleNotAllowed);
    }
    // Agent 憑證一定要有範圍（Worker 的 Run 或 Controller 的一輪）；沒有範圍的 Agent 憑證不能退回任何預設
    if record.role == Role::Agent && record.scope.is_none() && record.controller.is_none() {
        return Err(Denial::CredentialInvalid);
    }
    // Worker 與 Controller 各自只能用自己的工具（票 17）；Controller 的權限提示由處理函式一律拒絕
    if let Some(tool) = path.strip_prefix(AGENT_TOOLS_PATH) {
        let controller_tool = CONTROLLER_TOOLS.contains(&tool);
        if record.controller.is_some() && !controller_tool && tool != TOOL_PERMISSION_PROMPT {
            return Err(Denial::WorkerOnly);
        }
        if record.scope.is_some() && controller_tool {
            return Err(Denial::ControllerOnly);
        }
    }
    Ok(record)
}

/// Host 必須是 loopback 的 IP 字面值加上 Runtime 實際監聽的埠（防 DNS rebinding）。
fn host_allowed(host: &str, port: u16) -> bool {
    let (address, host_port) = match host.strip_prefix('[') {
        Some(rest) => match rest.split_once("]:") {
            Some((address, host_port)) => (address.parse::<Ipv6Addr>().map(|a| a.is_loopback()), host_port),
            None => return false,
        },
        None => match host.rsplit_once(':') {
            Some((address, host_port)) => (address.parse::<Ipv4Addr>().map(|a| a.is_loopback()), host_port),
            None => return false,
        },
    };
    address == Ok(true) && host_port.parse::<u16>() == Ok(port)
}

fn origin_allowed(origin: &str) -> bool {
    APP_ORIGINS.contains(&origin)
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() && !token.contains(' ')).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_must_be_a_loopback_ip_literal_with_the_runtime_port() {
        assert!(host_allowed("127.0.0.1:47821", 47821));
        assert!(host_allowed("[::1]:47821", 47821));

        for bad in [
            "127.0.0.1",
            "127.0.0.1:1",
            "127.0.0.1:47821x",
            "localhost:47821",
            "evil.example:47821",
            "127.0.0.1.evil.example:47821",
            "10.0.0.1:47821",
            "[::2]:47821",
            "[::1]",
            "[::1",
            "[127.0.0.1]:47821",
            "::1:47821",
            "",
        ] {
            assert!(!host_allowed(bad, 47821), "{bad:?} 不應通過");
        }
    }

    #[test]
    fn only_tauri_app_origins_are_allowed() {
        for ok in APP_ORIGINS {
            assert!(origin_allowed(ok));
        }
        for bad in ["null", "", "http://localhost:5173", "http://tauri.localhost.evil.example", "https://evil.example"] {
            assert!(!origin_allowed(bad), "{bad:?} 不應通過");
        }
    }

    fn headers_with_authorization(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, value.parse().unwrap());
        headers
    }

    #[test]
    fn bearer_token_is_extracted_only_from_a_well_formed_bearer_header() {
        assert_eq!(bearer_token(&headers_with_authorization("Bearer abc")), Some("abc"));
        assert_eq!(bearer_token(&headers_with_authorization("bearer abc")), Some("abc"));
        for bad in ["Basic abc", "Bearer", "Bearer ", "Bearer a b", "abc"] {
            assert_eq!(bearer_token(&headers_with_authorization(bad)), None, "{bad:?}");
        }
        assert_eq!(bearer_token(&HeaderMap::new()), None);
    }

    #[test]
    fn every_denial_has_a_distinct_code() {
        let all = [
            Denial::HostNotAllowed,
            Denial::PreflightRefused,
            Denial::OriginNotAllowed,
            Denial::ClientHeaderMissing,
            Denial::CredentialMissing,
            Denial::CredentialInvalid,
            Denial::CredentialExpired,
            Denial::RoleNotAllowed,
            Denial::Internal,
        ];
        let codes: std::collections::HashSet<_> = all.iter().map(|d| d.code()).collect();
        assert_eq!(codes.len(), all.len());
    }
}
