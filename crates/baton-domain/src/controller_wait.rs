//! Controller 事件等待（desktop-controller-cli 票 06，spec「事件等待」）：Controller CLI 派工後在背景執行
//! MCP shim 的 `wait` 子命令，它長輪詢 Agent API 的等待端點；有事件就印出摘要並結束，Claude Code 原生通知 Controller。
//!
//! 純邏輯、無 I/O。幾條規則：
//! - **事件來源是追加式事件表**：游標就是事件表的提交序號（`seq`）。回報四種事件：Worker Run 結束（成功、失敗、取消、中斷）、
//!   Task 進入受阻、裁定請求被回答、Controller 收到訊息（`controller_inbox` 聚合，送出訊息時在同一個交易中追加）。
//! - **游標依 Controller Run 保存**：等待端點回傳游標之後的事件；`wait` 印出摘要後才確認（`acknowledge`），
//!   確認過的事件不再回傳。確認只會往前，也不能超過已存在的事件。
//! - **新的 Controller Run 從哪裡開始**：接續同一個 Workspace 上一個 Controller Run 確認到的位置（換帳號、重開時不漏掉
//!   中間發生的事）；這個 Workspace 從沒有 Controller 等待過時，從它開啟的那一刻開始（不回報開啟前的歷史）。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::message::MessageKind;
use crate::{decision, run, task, Timestamp};

/// 等待端點（Agent API，只接受仍開著的 Controller 憑證）。
pub const WAIT_PATH: &str = "/agent/v1/controller/wait";
/// MCP shim 的等待子命令。
pub const WAIT_SUBCOMMAND: &str = "wait";
/// Controller 宿主把 shim 執行檔的完整路徑放在這個環境變數交給 `claude`，Controller 以 `"$BATON_MCP_SHIM" wait` 執行。
pub const SHIM_PATH_ENV: &str = "BATON_MCP_SHIM";

/// 沒有指定時等多久（秒）。
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 600;
/// 最多等多久（秒）。
pub const MAX_TIMEOUT_SECONDS: u64 = 3600;
/// 一次最多回傳的事件數；更多的留到下一次 `wait`。
pub const BATCH_LIMIT: usize = 20;

/// `wait` 子命令的結束碼（固定、互不相同）。
pub const EXIT_EVENTS: i32 = 0;
/// 連線失敗、Runtime 回應錯誤等其他失敗。
pub const EXIT_FAILURE: i32 = 1;
/// 參數錯誤或缺少環境變數。
pub const EXIT_USAGE: i32 = 2;
/// 憑證已失效（Controller CLI 已結束）：不要再執行 `wait`。
pub const EXIT_REVOKED: i32 = 3;
/// 等到逾時都沒有新事件：需要時再執行一次。
pub const EXIT_TIMED_OUT: i32 = 4;

/// Workspace 的 Controller 收件匣；`aggregate_id` 是 Workspace，版本是寄給 Controller 的訊息累計數。
pub const CONTROLLER_INBOX_AGGREGATE: &str = "controller_inbox";
pub const EVENT_CONTROLLER_MESSAGE_RECEIVED: &str = "controller_message_received";

/// 代表 Worker Run 結束的事件（交接不算：工作由接手的 Run 繼續）。
pub const RUN_ENDED_EVENTS: [&str; 4] =
    [run::EVENT_RUN_SUCCEEDED, run::EVENT_RUN_FAILED, run::EVENT_RUN_CANCELLED, run::EVENT_RUN_INTERRUPTED];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerEventKind {
    RunEnded,
    TaskBlocked,
    DecisionAnswered,
    MessageReceived,
}

impl ControllerEventKind {
    pub fn label(self) -> &'static str {
        match self {
            ControllerEventKind::RunEnded => "Run 結束",
            ControllerEventKind::TaskBlocked => "Task 受阻",
            ControllerEventKind::DecisionAnswered => "裁定已回答",
            ControllerEventKind::MessageReceived => "收到訊息",
        }
    }
}

/// Store 從事件表讀出、已確認屬於這個 Workspace 的一筆事件，加上摘要需要的關聯事實。
#[derive(Debug, Clone, PartialEq)]
pub struct EventFacts {
    pub seq: i64,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub event_type: String,
    pub occurred_at: Timestamp,
    pub payload: serde_json::Value,
    /// Run、Task 與裁定請求事件所屬的 Task。
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    /// 裁定請求的問題。
    pub question: Option<String>,
}

/// 回報給 Controller 的一筆事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerEvent {
    /// 事件表的提交序號；確認時送回最後一筆的值。
    pub cursor: i64,
    pub kind: ControllerEventKind,
    pub occurred_at: Timestamp,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub decision_request_id: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
    /// 給人與模型讀的一行摘要。
    pub summary: String,
}

/// 等待端點的回應。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitResponse {
    /// 游標之後的事件（依發生順序，最多 `BATCH_LIMIT` 筆）；逾時為空。
    pub events: Vec<ControllerEvent>,
    /// 有事件時是最後一筆的游標（印出後以它確認）；逾時時是目前已確認的位置。
    pub cursor: i64,
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WaitError {
    #[error("等待時間最多 {MAX_TIMEOUT_SECONDS} 秒")]
    TimeoutTooLong,
    #[error("確認的游標不能是負數，也不能超過已存在的事件")]
    CursorInvalid,
}

impl WaitError {
    pub fn code(&self) -> &'static str {
        match self {
            WaitError::TimeoutTooLong => "wait_timeout_invalid",
            WaitError::CursorInvalid => "wait_cursor_invalid",
        }
    }
}

/// 這次要等多久（秒）；`0` 表示只查一次、不等待。
pub fn timeout_seconds(requested: Option<u64>) -> Result<u64, WaitError> {
    match requested.unwrap_or(DEFAULT_TIMEOUT_SECONDS) {
        seconds if seconds > MAX_TIMEOUT_SECONDS => Err(WaitError::TimeoutTooLong),
        seconds => Ok(seconds),
    }
}

/// 確認之後的游標：只往前；不能是負數，也不能超過事件表目前最後一筆 `latest`。
pub fn acknowledge(stored: i64, ack: i64, latest: i64) -> Result<i64, WaitError> {
    if ack < 0 || ack > latest {
        return Err(WaitError::CursorInvalid);
    }
    Ok(stored.max(ack))
}

/// Controller Run 第一次等待時的游標：接續同一個 Workspace 先前的 Controller Run 確認到的位置（`carried`）；
/// 沒有的話從這個 Run 開啟的事件（`opened`）開始；都沒有時從現在（`latest`）開始。
pub fn initial_cursor(carried: Option<i64>, opened: Option<i64>, latest: i64) -> i64 {
    carried.or(opened).unwrap_or(latest)
}

/// 事件轉成回報給 Controller 的形式；不屬於四種事件的回 `None`。
pub fn describe(facts: &EventFacts) -> Option<ControllerEvent> {
    let text = |key: &str| facts.payload[key].as_str().filter(|s| !s.trim().is_empty()).map(str::to_string);
    let task = || match (&facts.task_title, &facts.task_id) {
        (Some(title), Some(id)) => format!("Task「{title}」（{id}）"),
        (None, Some(id)) => format!("Task {id}"),
        _ => "Task".to_string(),
    };
    let base = ControllerEvent {
        cursor: facts.seq,
        kind: ControllerEventKind::RunEnded,
        occurred_at: facts.occurred_at,
        task_id: facts.task_id.clone(),
        run_id: None,
        decision_request_id: None,
        message_id: None,
        summary: String::new(),
    };
    match (facts.aggregate_type.as_str(), facts.event_type.as_str()) {
        (run::RUN_AGGREGATE, event_type) if RUN_ENDED_EVENTS.contains(&event_type) => {
            let what = match event_type {
                run::EVENT_RUN_SUCCEEDED => "成功結束，成果待評估",
                run::EVENT_RUN_FAILED => "以失敗結束",
                run::EVENT_RUN_CANCELLED => "已取消",
                _ => "中斷",
            };
            let reason = match (event_type, text("reason")) {
                (run::EVENT_RUN_SUCCEEDED, _) | (_, None) => String::new(),
                (_, Some(reason)) => format!("：{reason}"),
            };
            Some(ControllerEvent {
                run_id: Some(facts.aggregate_id.clone()),
                summary: format!("{} 的 Worker Run {} {what}{reason}", task(), facts.aggregate_id),
                ..base
            })
        }
        (task::TASK_AGGREGATE, task::EVENT_STATUS_CHANGED) if facts.payload["to"] == "blocked" => {
            let reason = text("reason").map(|r| format!("（{r}）")).unwrap_or_default();
            Some(ControllerEvent {
                kind: ControllerEventKind::TaskBlocked,
                summary: format!("{} 受阻{reason}；請查看 Task 詳情決定下一步", task()),
                ..base
            })
        }
        (decision::DECISION_AGGREGATE, decision::EVENT_DECISION_ANSWERED) => {
            let question = facts.question.as_deref().map(|q| format!("「{q}」")).unwrap_or_default();
            let answer = [text("action"), text("option_id")].into_iter().flatten().collect::<Vec<_>>().join("：");
            let note = text("note").map(|n| format!("，備註：{n}")).unwrap_or_default();
            Some(ControllerEvent {
                kind: ControllerEventKind::DecisionAnswered,
                decision_request_id: Some(facts.aggregate_id.clone()),
                summary: format!("裁定請求{question}（{}）已被回答：{answer}{note}", facts.aggregate_id),
                ..base
            })
        }
        (CONTROLLER_INBOX_AGGREGATE, EVENT_CONTROLLER_MESSAGE_RECEIVED) => {
            let from = match (text("from_task_title"), text("from_task_id")) {
                (Some(title), Some(id)) => format!("Task「{title}」（{id}）"),
                (None, Some(id)) => format!("Task {id}"),
                _ => "Worker".to_string(),
            };
            let kind = text("kind").unwrap_or_default();
            let subject = text("subject").unwrap_or_default();
            Some(ControllerEvent {
                kind: ControllerEventKind::MessageReceived,
                task_id: text("from_task_id"),
                message_id: text("message_id"),
                summary: format!("收到 {from} 寄給 Controller 的訊息「{subject}」（{kind}）；以 read_inbox 讀取全文"),
                ..base
            })
        }
        _ => None,
    }
}

/// Worker 寄給 Controller 的訊息存入收件匣時追加的事件；`version` 是這個 Workspace 寄給 Controller 的訊息累計數。
/// `from` 是寄件的 (Task 識別, Task 標題)。
pub fn message_received_event(
    workspace_id: &str,
    version: i64,
    message_id: &str,
    kind: MessageKind,
    subject: &str,
    from: Option<(&str, &str)>,
    now: Timestamp,
) -> DomainEvent {
    DomainEvent {
        aggregate_type: CONTROLLER_INBOX_AGGREGATE,
        aggregate_id: workspace_id.to_string(),
        aggregate_version: version,
        event_type: EVENT_CONTROLLER_MESSAGE_RECEIVED,
        occurred_at: now,
        payload: serde_json::json!({
            "message_id": message_id,
            "kind": kind.as_str(),
            "subject": subject,
            "from_task_id": from.map(|(id, _)| id),
            "from_task_title": from.map(|(_, title)| title),
        }),
    }
}

/// `wait` 有事件時印出的文字。
pub fn render_events(events: &[ControllerEvent]) -> String {
    let mut text = format!("Baton 事件（{} 件）：\n", events.len());
    for event in events {
        text.push_str(&format!("- [{}] {}\n", event.kind.label(), event.summary));
    }
    text.push_str("以上事件已確認，不會再回報；處理後若仍有進行中的工作，再執行 wait 等待下一批事件。\n");
    text
}

#[cfg(test)]
mod tests;
