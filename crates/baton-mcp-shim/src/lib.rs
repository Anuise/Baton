//! MCP 工具 shim：Claude Code 以 stdio 啟動，shim 把 MCP `tools/call` 轉送到 Runtime 的 Agent API。
//!
//! 傳輸是逐行 JSON-RPC 2.0：stdout 只輸出協定訊息，日誌一律走 stderr。
//! Agent 憑證只存在於 [`ShimConfig`] 與送往 Runtime 的 `Authorization` 標頭，
//! 不會出現在 stdout、stderr、錯誤訊息或 `Debug` 輸出。

use std::fmt;
use std::io;
use std::time::Duration;

use baton_domain::agent::{
    AGENT_TOKEN_ENV, AGENT_TOOLS_PREFIX, BLOCKING_MAX_TASKS, DETAILS_MAX_BYTES, REASON_MAX_CHARS, RUNTIME_URL_ENV, SUMMARY_MAX_CHARS,
    TOOL_ADD_DEPENDENCY, TOOL_CANCEL_TASK, TOOL_CONFIRM_HANDOFF, TOOL_CONTROLLER_ACTION, TOOL_DISPATCH_TASK, TOOL_GET_TASK_CONTRACT,
    TOOL_PERMISSION_PROMPT, TOOL_PROPOSE_DECISION_REQUEST, TOOL_PROPOSE_TASK, TOOL_READ_INBOX, TOOL_REPORT_BLOCKED,
    TOOL_REPORT_PROGRESS, TOOL_REQUEST_DECISION, TOOL_SAVE_CHECKPOINT, TOOL_SEND_MESSAGE, TOOL_UPDATE_TASK,
};
use baton_domain::workdir::TaskKind;
use baton_domain::authority::Matter;
use baton_domain::handoff::CONFIRMATION_MAX_CHARS;
use baton_domain::message::{
    MessageKind, BODY_MAX_BYTES, ID_MAX_CHARS, INBOX_DEFAULT_LIMIT, INBOX_MAX_LIMIT, MAX_RECIPIENTS, SUBJECT_MAX_CHARS,
};
use baton_domain::artifact::{ARTIFACT_MAX_BYTES, DESCRIPTION_MAX_CHARS, NAME_MAX_CHARS, TOOL_REGISTER_ARTIFACT};
use baton_domain::knowledge::{
    BODY_MAX_CHARS, EVIDENCE_MAX, EVIDENCE_MAX_CHARS, LESSON_FIELD_MAX_CHARS, QUERY_MAX_CHARS, QUERY_TERM_MIN_CHARS,
    TAGS_MAX, TAG_MAX_CHARS, TITLE_MAX_CHARS, TOOL_RECORD_LESSON_CANDIDATE, TOOL_SEARCH_KNOWLEDGE,
};
use baton_domain::external_operation::{
    KEY_MAX_CHARS, OPERATION_MAX_CHARS, REASON_MAX_CHARS as OPERATION_REASON_MAX_CHARS, RECEIPT_MAX_CHARS,
    REQUEST_MAX_BYTES, TARGET_MAX_CHARS, TOOL_BEGIN_EXTERNAL_OPERATION, TOOL_RECORD_EXTERNAL_OPERATION_RESULT,
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
        {
            "name": TOOL_CONTROLLER_ACTION,
            "description": "Controller 在處理決策權限表列出的事項前先呼叫（這個工具本身不執行任何動作）。回 allowed：可以自主處理；回 requires_decision：這個事項必須由使用者裁定，動作被拒絕並已建立裁定請求，等待裁定、不要自行執行或繞過；回 approved：使用者已核准這一次（用過即失效，conditions 是使用者的條件）；回 denied：使用者沒有核准；回 human_only：只能由使用者發起。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "matter": {
                        "type": "string",
                        "enum": Matter::ALL.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
                        "description": "決策權限表中的事項",
                    },
                    "task_id": { "type": "string", "description": "動作的目標 Task；省略時是你自己的 Task" },
                    "summary": { "type": "string", "description": "要做什麼（一句話）" },
                    "reason": { "type": "string", "description": "為什麼要這麼做" },
                    "evidence": { "type": "array", "items": { "type": "string" }, "description": "支持判斷的證據" },
                    "urgency": { "type": "string", "enum": ["low", "normal", "high"], "description": "急迫程度" },
                },
                "required": ["matter", "summary", "reason"],
            },
        },
        {
            "name": TOOL_SEND_MESSAGE,
            "description": "寄訊息給同一個 Workspace 的其他 Task（收件人用 task id）。收件人要到下次讀取收件匣時才看得到，Runtime 不會中途插話；訊息不能改變任何 Task 的契約、範圍或歸屬；重送同一個 message_id 只會交付一次。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": MAX_RECIPIENTS,
                        "description": "收件 Task 的 task id（同一個 Workspace）",
                    },
                    "kind": {
                        "type": "string",
                        "enum": MessageKind::ALL.map(MessageKind::as_str),
                        "default": MessageKind::Note.as_str(),
                        "description": "訊息種類，預設 note",
                    },
                    "subject": { "type": "string", "maxLength": SUBJECT_MAX_CHARS, "description": "訊息主旨" },
                    "body": {
                        "type": "string",
                        "description": format!("訊息本文（上限 {} KiB）", BODY_MAX_BYTES / 1024),
                    },
                    "message_id": {
                        "type": "string",
                        "maxLength": ID_MAX_CHARS,
                        "description": "選填的冪等鍵；重送同一個 message_id 只會交付一次",
                    },
                    "in_reply_to": { "type": "string", "description": "選填，要回覆的 message_id" },
                    "correlation_id": { "type": "string", "maxLength": ID_MAX_CHARS, "description": "選填的關聯 id，用來串起同一串往來" },
                },
                "required": ["to", "subject"],
            },
        },
        {
            "name": TOOL_READ_INBOX,
            "description": "讀取寄給這個 Task 的訊息；回傳的未讀訊息會被標為已讀。建議在 save_checkpoint 與開始新階段前呼叫。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "include_read": { "type": "boolean", "default": false, "description": "是否一併回傳已讀訊息" },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": INBOX_MAX_LIMIT,
                        "default": INBOX_DEFAULT_LIMIT,
                        "description": "最多回傳幾封",
                    },
                },
            },
        },
        {
            "name": TOOL_REGISTER_ARTIFACT,
            "description": "登記一個成果（報告、檔案等交付物）；同名再次登記會追加新版本。content 與 content_base64 擇一提供。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "maxLength": NAME_MAX_CHARS,
                        "description": "成果檔名，不可含路徑分隔字元",
                    },
                    "description": { "type": "string", "maxLength": DESCRIPTION_MAX_CHARS, "description": "成果的簡短描述（選填）" },
                    "content": {
                        "type": "string",
                        "description": format!("UTF-8 文字內容；與 content_base64 擇一（上限 {} MiB）", ARTIFACT_MAX_BYTES / (1024 * 1024)),
                    },
                    "content_base64": {
                        "type": "string",
                        "description": format!(
                            "Base64 編碼的二進位內容；與 content 擇一（解碼後上限 {} MiB）",
                            ARTIFACT_MAX_BYTES / (1024 * 1024)
                        ),
                    },
                },
                "required": ["name"],
            },
        },
        {
            "name": TOOL_RECORD_LESSON_CANDIDATE,
            "description": "提交一則候選教訓（踩過的坑、根因與解法）。提交後只是「候選」，不會被當作已驗證的知識，                需使用者在知識分頁確認後才會生效；適用範圍由這個 Run 的 Task 決定。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "maxLength": TITLE_MAX_CHARS, "description": "教訓的簡短標題" },
                    "body": { "type": "string", "maxLength": BODY_MAX_CHARS, "description": "詳細說明（選填）" },
                    "trigger": {
                        "type": "string",
                        "maxLength": LESSON_FIELD_MAX_CHARS,
                        "description": "觸發條件：什麼情況下會再犯（選填）",
                    },
                    "root_cause": {
                        "type": "string",
                        "maxLength": LESSON_FIELD_MAX_CHARS,
                        "description": "根因或待驗假說（選填）",
                    },
                    "resolution": {
                        "type": "string",
                        "maxLength": LESSON_FIELD_MAX_CHARS,
                        "description": "有效解法或禁用做法（選填）",
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string", "maxLength": TAG_MAX_CHARS },
                        "maxItems": TAGS_MAX,
                        "description": "標籤（選填）",
                    },
                    "evidence": {
                        "type": "array",
                        "items": { "type": "string", "maxLength": EVIDENCE_MAX_CHARS },
                        "maxItems": EVIDENCE_MAX,
                        "description": "證據：成果、Run、事件或外部來源的參照與說明（選填）",
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["task", "goal", "workspace"],
                        "default": "task",
                        "description": "適用範圍：這個 Task（預設）、所屬 Goal 或整個 Workspace",
                    },
                },
                "required": ["title"],
            },
        },
        {
            "name": TOOL_SEARCH_KNOWLEDGE,
            "description": "搜尋這個 Run 適用範圍內「已驗證且有效」的決策、教訓與知識（候選、已撤銷或過期的不會出現）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "maxLength": QUERY_MAX_CHARS,
                        "description": format!(
                            "以空白分詞，每個詞至少 {QUERY_TERM_MIN_CHARS} 個字元，全部的詞都要出現；空字串列出全部適用的有效條目"
                        ),
                    },
                },
            },
        },
        {
            "name": TOOL_BEGIN_EXTERNAL_OPERATION,
            "description": "送出 Runtime 無法撤回的外部操作（部署、發訊、付費、遠端寫入、修改正式資料）之前必須先呼叫，以冪等鍵登記。只有回應 status 為 proceed 才可以送出；already_succeeded 代表先前已經成功，沿用 receipt、不要再送；result_unknown 或 in_progress_elsewhere 不要送出，先保存 checkpoint 再 report_blocked 結束，或改做其他步驟。重試同一件事（包括換 Run 或 Runtime 重啟後）一律用同一個冪等鍵。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "idempotency_key": {
                        "type": "string",
                        "maxLength": KEY_MAX_CHARS,
                        "description": "代表「這一件事」的穩定鍵（例如 deploy-web-v1.2.3）；同一件事永遠用同一個鍵，不同的事用不同的鍵",
                    },
                    "operation": { "type": "string", "maxLength": OPERATION_MAX_CHARS, "description": "操作種類，例如 deploy、send_email、payment" },
                    "target": { "type": "string", "maxLength": TARGET_MAX_CHARS, "description": "目標資源，例如 production、收件人或帳戶" },
                    "request": {
                        "type": "object",
                        "description": format!("影響結果的參數（上限 {} KiB）；與操作、目標一起計算要求雜湊，同一個鍵的內容必須相同", REQUEST_MAX_BYTES / 1024),
                    },
                },
                "required": ["idempotency_key", "operation", "target"],
            },
        },
        {
            "name": TOOL_RECORD_EXTERNAL_OPERATION_RESULT,
            "description": "回報已登記的外部操作結果。確定成功回 succeeded 並附回執；確定沒有生效回 failed；逾時、連線中斷或任何無法確定是否已生效的情形一律回 unknown（會建立裁定請求、暫停這個 Task，等使用者查證），不要自行重送。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "idempotency_key": { "type": "string", "maxLength": KEY_MAX_CHARS, "description": "登記時使用的冪等鍵" },
                    "result": { "type": "string", "enum": ["succeeded", "failed", "unknown"], "description": "結果" },
                    "receipt": { "type": "string", "maxLength": RECEIPT_MAX_CHARS, "description": "succeeded 時必填：外部系統的回執（例如部署編號、訊息識別）" },
                    "reason": { "type": "string", "maxLength": OPERATION_REASON_MAX_CHARS, "description": "failed 或 unknown 時必填：原因" },
                },
                "required": ["idempotency_key", "result"],
            },
        },
        {
            "name": TOOL_CONFIRM_HANDOFF,
            "description": "你是交接的接手者時（Prompt 中有「交接」段落），開始工作前先核對檢查點與工作目錄、成果的實際狀態，再呼叫這個工具確認接手：已完成、未完成與結果不明的副作用。不是接手者時會被拒絕。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "completed": { "type": "string", "maxLength": CONFIRMATION_MAX_CHARS, "description": "核對後確認已完成的部分" },
                    "incomplete": { "type": "string", "maxLength": CONFIRMATION_MAX_CHARS, "description": "尚未完成、接下來要做的部分" },
                    "unknown_side_effects": {
                        "type": "string",
                        "maxLength": CONFIRMATION_MAX_CHARS,
                        "description": "結果不明的副作用（例如不確定是否已送出的外部操作）；沒有就寫「無」",
                    },
                },
                "required": ["completed", "incomplete", "unknown_side_effects"],
            },
        },
        {
            "name": TOOL_PROPOSE_TASK,
            "description": "Controller 專屬（Worker 呼叫會被拒絕，controller_only）：在這個 Workspace 建立一個新的 Task。建立的 Task 一律是 pending，與使用者手動建立的 Task 受相同的准入檢查，之後要用 dispatch_task 才會派工。goal_id 與 goal_title 必須剛好提供一個。資訊不足時把契約欄位標為 unknown，不要虛構內容。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Task 標題" },
                    "goal_id": { "type": "string", "description": "歸屬的既有 Goal；與 goal_title 二擇一" },
                    "goal_title": { "type": "string", "description": "建立新 Goal 並歸屬其下；與 goal_id 二擇一" },
                    "kind": {
                        "type": "string",
                        "enum": TaskKind::ALL.map(TaskKind::as_str),
                        "default": TaskKind::default().as_str(),
                        "description": "Task 種類：development（程式開發）、research（研究，預設）、document（文件）；建立後不可變更",
                    },
                    "contract": {
                        "type": "object",
                        "description": CONTRACT_DESCRIPTION,
                    },
                },
                "required": ["title"],
            },
        },
        {
            "name": TOOL_UPDATE_TASK,
            "description": "Controller 專屬（Worker 呼叫會被拒絕，controller_only）：以新契約整份取代 Task 的契約（內容相同不會新增版本）。已派工過的 Task 修改完成條件（done_criteria）必須由使用者裁定，Runtime 會回 requires_decision，等待裁定、不要繞過。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "要修改的 Task" },
                    "contract": { "type": "object", "description": CONTRACT_DESCRIPTION },
                    "note": { "type": "string", "description": "這次修改的說明（選填）" },
                },
                "required": ["task_id", "contract"],
            },
        },
        {
            "name": TOOL_ADD_DEPENDENCY,
            "description": "Controller 專屬（Worker 呼叫會被拒絕，controller_only）：讓 task_id 依賴 depends_on（depends_on 完成前 task_id 不會被派出）。重複新增是冪等的；Runtime 拒絕自我相依與造成循環的相依。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "要等待的 Task" },
                    "depends_on": { "type": "string", "description": "被依賴、必須先完成的 Task" },
                },
                "required": ["task_id", "depends_on"],
            },
        },
        {
            "name": TOOL_DISPATCH_TASK,
            "description": "Controller 專屬（Worker 呼叫會被拒絕，controller_only）：派工一個 Task。pending 的 Task 會先做完整准入檢查再派工，准入不過回 task_admission_denied（附缺項）；超出 Workspace 授權時被拒絕並自動建立裁定請求（requires_decision），等使用者裁定、不要繞過。依賴未回答裁定請求的 Task 不會被派出。成功時回應 status 是 started（已開始執行）或 queued（已排入佇列）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "要派工的 Task" },
                    "priority": { "type": "integer", "description": "優先順序（選填），數字越大越優先" },
                },
                "required": ["task_id"],
            },
        },
        {
            "name": TOOL_CANCEL_TASK,
            "description": "Controller 專屬（Worker 呼叫會被拒絕，controller_only）：取消一個 Task；取消後不能再改契約、加相依或派工。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "要取消的 Task" },
                },
                "required": ["task_id"],
            },
        },
        {
            "name": TOOL_PROPOSE_DECISION_REQUEST,
            "description": "Controller 專屬（Worker 呼叫會被拒絕，controller_only）：向使用者提出裁定請求，提供 2 到 8 個選項與取捨。只問會阻擋下一步的關鍵問題。列在 blocking 的 Task 在使用者回答前不會被派出；不依賴這個答案的工作可以照常派出。使用者回答後你會在對話中收到工作事件，不要自行假設答案。",
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
                    "blocking": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": BLOCKING_MAX_TASKS,
                        "description": "依賴這個答案的 Task id；使用者回答前這些 Task 不會被派出",
                    },
                },
                "required": ["question", "reason", "options", "blocking"],
            },
        },
    ])
}

/// propose_task 與 update_task 的契約欄位說明（六項最小契約，見 ADR-0011）。
const CONTRACT_DESCRIPTION: &str = "六項最小契約：objective（目標）、inputs（輸入與依據）、scope（範圍與限制）、deliverables（預期成果）、done_criteria（完成與驗證條件）、blocker_handling（阻礙處理），每項是下列之一：{\"status\":\"filled\",\"text\":…,\"references\"?:[{\"kind\",\"target\",\"version\"}]}（已填寫）、{\"status\":\"not_applicable\",\"reason\":…}（不適用，必須附理由；objective、done_criteria 不可標不適用）、{\"status\":\"unknown\",\"note\"?:…}（未知，補足前無法派工）。另有 required_capabilities：這個 Task 需要的 Workspace 授權能力（字串陣列）。資訊不足時標 unknown，不要虛構。";

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
        TOOL_CONTROLLER_ACTION,
        TOOL_SEND_MESSAGE,
        TOOL_READ_INBOX,
        TOOL_REGISTER_ARTIFACT,
        TOOL_RECORD_LESSON_CANDIDATE,
        TOOL_SEARCH_KNOWLEDGE,
        TOOL_BEGIN_EXTERNAL_OPERATION,
        TOOL_RECORD_EXTERNAL_OPERATION_RESULT,
        TOOL_CONFIRM_HANDOFF,
        TOOL_PROPOSE_TASK,
        TOOL_UPDATE_TASK,
        TOOL_ADD_DEPENDENCY,
        TOOL_DISPATCH_TASK,
        TOOL_CANCEL_TASK,
        TOOL_PROPOSE_DECISION_REQUEST,
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
