//! Controller 事件等待（`wait` 子命令）：Controller 在背景 Bash 執行 `"$BATON_MCP_SHIM" wait [--timeout <秒>]`，
//! shim 長輪詢 Runtime 的等待端點；有事件就把摘要印到 stdout、確認後以 0 結束，讓 Claude Code 通知 Controller。
//!
//! shim 只轉送、不做判斷也不保存狀態。憑證只從環境變數取得，只出現在送往 Runtime 的 `Authorization` 標頭。

use std::ffi::OsString;
use std::io::Write;
use std::time::Duration;

use baton_domain::controller_wait::{
    render_events, timeout_seconds, WaitResponse, EXIT_EVENTS, EXIT_FAILURE, EXIT_REVOKED, EXIT_TIMED_OUT, EXIT_USAGE,
    WAIT_PATH,
};
use serde_json::{json, Value};

use crate::{ShimConfig, CLIENT_HEADER, CLIENT_HEADER_VALUE, REQUEST_TIMEOUT};

const PREFIX: &str = "baton-mcp-shim wait";
const USAGE: &str = "用法：baton-mcp-shim wait [--timeout <秒>]";
/// Runtime 最多等 `timeout_seconds` 秒才回應；HTTP 用戶端在那之上多留的餘裕。
const RESPONSE_GRACE: Duration = Duration::from_secs(30);

/// 呼叫等待端點失敗的原因；說明不含憑證。
enum Failure {
    /// 401：憑證已失效（Controller CLI 已結束）。
    Revoked,
    Failed(String),
}

/// 執行 `wait`：`args` 是 `wait` 之後的參數，事件摘要與逾時說明寫到 `out`（stdout），其餘訊息走 stderr。回傳結束碼。
pub async fn run_wait(args: &[OsString], lookup: impl Fn(&str) -> Option<String>, out: &mut impl Write) -> i32 {
    let timeout = match parse_timeout(args) {
        Ok(timeout) => timeout,
        Err(reason) => {
            eprintln!("{PREFIX}：{reason}\n{USAGE}");
            return EXIT_USAGE;
        }
    };
    // 錯誤訊息只含變數名稱，不含憑證
    let config = match ShimConfig::from_lookup(lookup) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{PREFIX}：{error}");
            return EXIT_USAGE;
        }
    };
    // 逾時依請求設定：等待請求要比伺服器的等待時間長，確認請求用一般逾時
    let client = match reqwest::Client::builder().no_proxy().build() {
        Ok(client) => client,
        Err(_) => {
            eprintln!("{PREFIX}：無法建立 HTTP 用戶端");
            return EXIT_FAILURE;
        }
    };

    let wait_timeout = Duration::from_secs(timeout) + RESPONSE_GRACE;
    let bytes = match post(&client, &config, json!({ "timeout_seconds": timeout }), wait_timeout).await {
        Ok(bytes) => bytes,
        Err(Failure::Revoked) => {
            eprintln!("{PREFIX}：Controller 憑證已失效（Controller CLI 已結束），不要再執行 wait");
            return EXIT_REVOKED;
        }
        Err(Failure::Failed(reason)) => {
            eprintln!("{PREFIX}：等待事件失敗（{reason}）");
            return EXIT_FAILURE;
        }
    };
    let response: WaitResponse = match serde_json::from_slice(&bytes) {
        Ok(response) => response,
        Err(_) => {
            eprintln!("{PREFIX}：無法解析 Runtime 的回應");
            return EXIT_FAILURE;
        }
    };

    if response.timed_out {
        let text = format!("等待逾時：{timeout} 秒內沒有新事件；需要時再執行 wait。\n");
        if write_out(out, &text).is_err() {
            eprintln!("{PREFIX}：無法寫出逾時說明");
            return EXIT_FAILURE;
        }
        return EXIT_TIMED_OUT;
    }

    // 摘要確實寫出後才確認；寫不出去就不確認，事件下次會再回報
    if write_out(out, &render_events(&response.events)).is_err() {
        eprintln!("{PREFIX}：無法寫出事件摘要，未確認，事件下次會再回報");
        return EXIT_FAILURE;
    }
    let ack = json!({ "ack": response.cursor, "timeout_seconds": 0 });
    match post(&client, &config, ack, REQUEST_TIMEOUT).await {
        Ok(_) => {}
        Err(Failure::Revoked) => eprintln!("{PREFIX}：警告：確認事件失敗（憑證已失效），這些事件下次可能再回報"),
        Err(Failure::Failed(reason)) => eprintln!("{PREFIX}：警告：確認事件失敗（{reason}），這些事件下次會再回報"),
    }
    EXIT_EVENTS
}

/// 解析 `[--timeout <秒>]`；沒給時用預設值。
fn parse_timeout(args: &[OsString]) -> Result<u64, String> {
    let mut requested = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg != "--timeout" {
            return Err(format!("不認得的參數 {}", arg.to_string_lossy()));
        }
        let value = args.next().ok_or("--timeout 缺少秒數")?;
        let seconds = value
            .to_str()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| format!("--timeout 必須是非負整數秒數，收到 {}", value.to_string_lossy()))?;
        requested = Some(seconds);
    }
    timeout_seconds(requested).map_err(|error| error.to_string())
}

fn write_out(out: &mut impl Write, text: &str) -> std::io::Result<()> {
    out.write_all(text.as_bytes())?;
    out.flush()
}

async fn post(client: &reqwest::Client, config: &ShimConfig, body: Value, timeout: Duration) -> Result<Vec<u8>, Failure> {
    let url = format!("{}{WAIT_PATH}", config.runtime_url.trim_end_matches('/'));
    let response = client
        .post(url)
        .bearer_auth(&config.token)
        .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
        .timeout(timeout)
        .json(&body)
        .send()
        .await
        .map_err(describe)?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(Failure::Revoked);
    }
    let bytes = response.bytes().await.map_err(describe)?;
    if !status.is_success() {
        // 錯誤回應形如 {"error":"code","message":"..."}；只帶出錯誤代碼
        let code = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|body| body.get("error").and_then(Value::as_str).map(str::to_string));
        return Err(Failure::Failed(match code {
            Some(code) => format!("Runtime 回應 HTTP {}：{code}", status.as_u16()),
            None => format!("Runtime 回應 HTTP {}", status.as_u16()),
        }));
    }
    Ok(bytes.to_vec())
}

fn describe(error: reqwest::Error) -> Failure {
    if error.is_timeout() {
        Failure::Failed("Runtime 回應逾時".to_string())
    } else {
        Failure::Failed("無法連線到 Runtime".to_string())
    }
}
