//! MCP 工具 shim：Claude Code 以 stdio 啟動，shim 把 MCP `tools/call` 轉送到 Runtime 的 Agent API。
//!
//! 傳輸是逐行 JSON-RPC 2.0：stdout 只輸出協定訊息，日誌一律走 stderr。
//! Agent 憑證只存在於 [`ShimConfig`] 與送往 Runtime 的 `Authorization` 標頭，
//! 不會出現在 stdout、stderr、錯誤訊息或 `Debug` 輸出。

use std::fmt;
use std::io;
use std::time::Duration;

use baton_domain::agent::{
    AGENT_TOKEN_ENV, AGENT_TOOLS_PREFIX, DETAILS_MAX_BYTES, REASON_MAX_CHARS, RUNTIME_URL_ENV, SUMMARY_MAX_CHARS,
    TOOL_GET_TASK_CONTRACT, TOOL_PERMISSION_PROMPT, TOOL_REPORT_BLOCKED, TOOL_REPORT_PROGRESS, TOOL_REQUEST_DECISION,
    TOOL_SAVE_CHECKPOINT,
};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// 每個請求都必須帶的固定標頭（與 Runtime 的入口防護一致）。
const CLIENT_HEADER: &str = "x-baton-client";
const CLIENT_HEADER_VALUE: &str = "baton-desktop";

const SUPPORTED_PROTOCOLS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
const FALLBACK_PROTOCOL: &str = "2025-06-18";
/// 目標是 loopback 上的 Runtime，整體逾時不必太長。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("缺少環境變數 {0}")]
    Missing(&'static str),
}

/// shim 的設定：Runtime 的 loopback 位址與 Agent 憑證。
#[derive(Clone)]
pub struct ShimConfig {
    pub runtime_url: String,
    pub token: String,
}

impl ShimConfig {
    pub fn new(runtime_url: String, token: String) -> Self {
        Self { runtime_url, token }
    }

    /// 由環境變數建立設定；缺少或空白的變數視為錯誤，錯誤訊息只寫變數名稱。
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |name: &'static str| lookup(name).filter(|v| !v.trim().is_empty()).ok_or(ConfigError::Missing(name));
        Ok(Self::new(get(RUNTIME_URL_ENV)?, get(AGENT_TOKEN_ENV)?))
    }
}

// 手寫 Debug 以遮蔽憑證
impl fmt::Debug for ShimConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShimConfig").field("runtime_url", &self.runtime_url).field("token", &"<redacted>").finish()
    }
}

/// 讀 `input` 的每一行 JSON-RPC 訊息並把回應寫到 `output`，直到 `input` 結束。
pub async fn serve<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    mut input: R,
    mut output: W,
    config: ShimConfig,
) -> io::Result<()> {
    let client = reqwest::Client::builder().timeout(REQUEST_TIMEOUT).no_proxy().build().map_err(io::Error::other)?;
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_line(&client, &config, &line).await {
            output.write_all(format!("{reply}\n").as_bytes()).await?;
            output.flush().await?;
        }
    }
}

/// 處理一行訊息；通知（沒有 id）不回應。
async fn handle_line(client: &reqwest::Client, config: &ShimConfig, line: &str) -> Option<Value> {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Some(rpc_error(Value::Null, PARSE_ERROR, "無法解析 JSON"));
    };
    let Some(message) = message.as_object() else {
        return Some(rpc_error(Value::Null, INVALID_REQUEST, "請求必須是 JSON 物件"));
    };
    let id = message.get("id")?.clone();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Some(rpc_error(id, INVALID_REQUEST, "缺少 method"));
    };
    let params = message.get("params").unwrap_or(&Value::Null);

    let outcome = match method {
        "initialize" => Ok(initialize_result(params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(client, config, params).await,
        _ => Err((METHOD_NOT_FOUND, format!("不支援的方法：{method}"))),
    };
    Some(match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, text)) => rpc_error(id, code, &text),
    })
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize_result(params: &Value) -> Value {
    let protocol = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .filter(|requested| SUPPORTED_PROTOCOLS.contains(requested))
        .unwrap_or(FALLBACK_PROTOCOL);
    json!({
        "protocolVersion": protocol,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "baton-mcp-shim", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": TOOL_GET_TASK_CONTRACT,
            "description": "取得這個 Run 的 Task 契約（目標、輸入、範圍、交付物、完成條件、卡住時的處理）。開始工作前先呼叫。",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": TOOL_REPORT_PROGRESS,
            "description": "回報目前進度；同時會續約這個 Run 的存活租約，卡住前應定期呼叫。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "summary": { "type": "string", "maxLength": SUMMARY_MAX_CHARS, "description": "目前進度的簡短摘要" },
                },
                "required": ["summary"],
            },
        },
        {
            "name": TOOL_SAVE_CHECKPOINT,
            "description": "保存可接續的檢查點，換帳號或中斷後由接手者使用。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "summary": { "type": "string", "maxLength": SUMMARY_MAX_CHARS, "description": "已完成與下一步的簡短摘要" },
                    "details": {
                        "type": "string",
                        "description": format!("接手者需要的細節（上限 {} KiB）", DETAILS_MAX_BYTES / 1024),
                    },
                },
                "required": ["summary"],
            },
        },
        {
            "name": TOOL_REPORT_BLOCKED,
            "description": "回報無法繼續的阻礙；回報前應先 save_checkpoint，之後結束工作。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "reason": { "type": "string", "maxLength": REASON_MAX_CHARS, "description": "無法繼續的原因" },
                },
                "required": ["reason"],
            },
        },
        {
            "name": TOOL_REQUEST_DECISION,
            "description": "遇到需要使用者裁定的問題（你無法自行決定時）才呼叫，並提供 2 到 8 個選項與取捨。呼叫後只有受影響的工作會暫停，不會影響其他工作；使用者裁定前不要自行假設答案。之後請先 save_checkpoint 再 report_blocked 結束，或改做不需要這項裁定的其他步驟。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "要使用者裁定的問題" },
                    "reason": { "type": "string", "description": "為什麼你無法自行決定" },
                    "options": {
                        "type": "array",
                        "minItems": 2,
                        "maxItems": 8,
                        "description": "可選的選項",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": "選項代碼（在這個請求內唯一）" },
                                "label": { "type": "string", "description": "給使用者看的選項說明" },
                                "tradeoff": { "type": "string", "description": "選這個選項的取捨" },
                            },
                            "required": ["id", "label"],
                        },
                    },
                    "recommendation": { "type": "string", "description": "你建議的選項 id" },
                    "evidence": { "type": "array", "items": { "type": "string" }, "description": "支持判斷的證據" },
                    "urgency": { "type": "string", "enum": ["low", "normal", "high"], "description": "急迫程度" },
                    "deadline_minutes": {
                        "type": "integer",
                        "description": "從現在起幾分鐘內需要答案；只用來標示逾期，逾時不代表同意",
                    },
                },
                "required": ["question", "reason", "options"],
            },
        },
        {
            "name": TOOL_PERMISSION_PROMPT,
            "description": "給 Claude Code 的 --permission-prompt-tool 使用，不是給模型主動呼叫的工具。回傳 {behavior: allow|deny, ...}：授權範圍內直接允許；範圍外會建立裁定請求並先回 deny，等使用者裁定。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tool_name": { "type": "string", "description": "要求權限的工具名稱" },
                    "input": { "type": "object", "description": "該工具這次的輸入" },
                    "tool_use_id": { "type": "string", "description": "這次工具呼叫的 id" },
                },
                "required": ["tool_name"],
            },
        },
    ])
}

/// 處理 `tools/call`：協定層的錯誤回 `Err`，Runtime 的結果（含失敗）一律包成 MCP 工具結果。
async fn call_tool(client: &reqwest::Client, config: &ShimConfig, params: &Value) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "缺少工具名稱 name".to_string()))?;
    let known = [
        TOOL_GET_TASK_CONTRACT,
        TOOL_REPORT_PROGRESS,
        TOOL_SAVE_CHECKPOINT,
        TOOL_REPORT_BLOCKED,
        TOOL_REQUEST_DECISION,
        TOOL_PERMISSION_PROMPT,
    ];
    if !known.contains(&name) {
        return Err((INVALID_PARAMS, format!("未知的工具：{name}")));
    }
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(arguments @ Value::Object(_)) => arguments.clone(),
        Some(_) => return Err((INVALID_PARAMS, "arguments 必須是物件".to_string())),
    };

    let (text, is_error) = match forward(client, config, name, &arguments).await {
        Ok(text) => (text, false),
        Err(text) => (text, true),
    };
    Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
}

/// 轉送到 Runtime：成功回傳回應原文，失敗回傳給模型看的說明（不含憑證）。
async fn forward(client: &reqwest::Client, config: &ShimConfig, tool: &str, arguments: &Value) -> Result<String, String> {
    let url = format!("{}{AGENT_TOOLS_PREFIX}/{tool}", config.runtime_url.trim_end_matches('/'));
    let response = client
        .post(url)
        .bearer_auth(&config.token)
        .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
        .json(arguments)
        .send()
        .await
        .map_err(|e| if e.is_timeout() { "Runtime 回應逾時".to_string() } else { "無法連線到 Runtime".to_string() })?;
    let status = response.status();
    let body = response.text().await.map_err(|_| "讀取 Runtime 回應失敗".to_string())?;
    let parsed = serde_json::from_str::<Value>(&body);

    if status.is_success() {
        return parsed.map(|_| body).map_err(|_| format!("Runtime 回應不是 JSON（HTTP {}）", status.as_u16()));
    }
    let field = |name: &str| parsed.as_ref().ok().and_then(|v| v.get(name)).and_then(Value::as_str).map(str::to_string);
    Err(match (field("error"), field("message")) {
        (Some(code), Some(message)) => format!("{code}：{message}（HTTP {}）", status.as_u16()),
        _ => format!("Runtime 回應錯誤（HTTP {}）", status.as_u16()),
    })
}
