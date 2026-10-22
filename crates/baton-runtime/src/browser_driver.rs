//! 正式的 BrowserDriver（票 46）：以受驗專案的 Playwright 執行瀏覽器操作。
//!
//! 每個 Session 一個 Node 程序，執行內嵌的 Runner 腳本（`playwright/runner.mjs`，`open` 時寫到暫存目錄），
//! 以 stdin／stdout 的 JSON 行協定溝通：每個要求一行、每個回應一行，回應帶回要求的 `id`。
//! 逾時的要求留在 Runner 裡繼續做，它晚到的回應以 `id` 辨認後略過。
//!
//! 秘密：`Fill` 的文字只經 stdin 交給 Runner，不出現在指令列、檔案或日誌中；Runner 的標準錯誤直接丟棄
//! （Playwright 的除錯輸出可能含輸入的文字），錯誤訊息回傳前去掉輸入的文字。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use baton_ports::{
    BrowserCommand, BrowserDriver, BrowserFile, BrowserOutcome, BrowserSession, BrowserSessionRequest, BrowserStep,
    PageSnapshot, PortError, PortResult,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// Runner 腳本；`open` 時寫到 Session 的暫存目錄再以 `node` 執行。
const RUNNER_SCRIPT: &str = include_str!("../playwright/runner.mjs");
const RUNNER_FILE: &str = "runner.mjs";
const TRACE_FILE: &str = "trace.zip";
const VIDEO_DIR: &str = "video";
/// 啟動瀏覽器的等待上限。
const OPEN_TIMEOUT: Duration = Duration::from_secs(60);
/// 步驟的寬限：操作本身以 `BrowserStep::timeout` 為限，之後的觀察（等待載入、截圖）在 Runner 內各有 10 秒上限。
const STEP_GRACE: Duration = Duration::from_secs(30);
/// 停止 trace、寫完錄影並關閉瀏覽器的等待上限。
const CLOSE_TIMEOUT: Duration = Duration::from_secs(60);
/// 秘密在錯誤訊息中被換成這個標記（與 Runner 相同）。
const REDACTED: &str = "***";

pub struct LocalBrowserDriver {
    /// Node.js 執行檔（CreateProcessW 依 PATH 尋找）。
    program: String,
    sessions: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Runner>>>>,
    /// 各 Session 實際使用的引擎（Runner 開啟時回報；票 48）。
    engines: Mutex<HashMap<String, String>>,
    next_session: AtomicU64,
    /// Session 識別的前綴：每個 Runtime 實例不同，承接者才能分辨一個 Session 是不是目前的 Runtime 開啟的（票 49）。
    prefix: String,
}

impl Default for LocalBrowserDriver {
    fn default() -> Self {
        Self {
            program: "node".to_string(),
            sessions: Mutex::default(),
            engines: Mutex::default(),
            next_session: AtomicU64::new(1),
            prefix: format!("browser-{}-", crate::verification_driver::instance_tag()),
        }
    }
}

fn failed(message: String) -> PortError {
    PortError::Failed(message)
}

/// 一個 Session 的 Runner 程序；被丟棄時程序隨之終止（`kill_on_drop`）。
struct Runner {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl Runner {
    /// 送出一個要求並等它的回應；`None` 表示在 `wait` 內沒有回應。
    async fn request(&mut self, mut request: Value, wait: Duration) -> PortResult<Option<Value>> {
        self.next_id += 1;
        let id = self.next_id;
        request["id"] = json!(id);
        let mut line = serde_json::to_vec(&request).map_err(|e| failed(format!("無法編碼瀏覽器要求：{e}")))?;
        line.push(b'\n');
        // 寫入失敗的訊息只描述管道，不含要求內容
        let gone = |e: std::io::Error| failed(format!("瀏覽器 Runner 已結束：{e}"));
        self.stdin.write_all(&line).await.map_err(gone)?;
        self.stdin.flush().await.map_err(gone)?;
        let stdout = &mut self.stdout;
        let read = async {
            loop {
                let Some(text) = stdout.next_line().await.map_err(|e| failed(format!("無法讀取瀏覽器 Runner 的輸出：{e}")))? else {
                    return Err(failed("瀏覽器 Runner 意外結束".to_string()));
                };
                // 不是協定的行、或是先前逾時要求晚到的回應：略過
                let Ok(response) = serde_json::from_str::<Value>(&text) else { continue };
                if response.get("id").and_then(Value::as_u64) == Some(id) {
                    return Ok(response);
                }
            }
        };
        match tokio::time::timeout(wait, read).await {
            Ok(response) => response.map(Some),
            Err(_) => Ok(None),
        }
    }
}

fn message_of(response: &Value) -> String {
    response.get("message").and_then(Value::as_str).unwrap_or("瀏覽器 Runner 回報失敗").to_string()
}

impl LocalBrowserDriver {
    fn session(&self, session: &BrowserSession) -> Option<Arc<tokio::sync::Mutex<Runner>>> {
        self.sessions.lock().expect("Session 表不會中毒").get(&session.0).cloned()
    }

    async fn spawn(&self, request: &BrowserSessionRequest, dir: &Path) -> PortResult<Runner> {
        let script = dir.join(RUNNER_FILE);
        tokio::fs::create_dir_all(dir).await.map_err(|e| failed(format!("無法建立 {}：{e}", dir.display())))?;
        tokio::fs::write(&script, RUNNER_SCRIPT).await.map_err(|e| failed(format!("無法寫入 {}：{e}", script.display())))?;
        let mut command = tokio::process::Command::new(&self.program);
        command
            .arg(&script)
            .current_dir(&request.working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = command
            .spawn()
            .map_err(|e| failed(format!("無法啟動 Node.js（{}）：{e}；web 驗證需要 Node.js 與受驗專案的 Playwright", self.program)))?;
        let stdin = child.stdin.take().expect("stdin 已設為 piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout 已設為 piped")).lines();
        Ok(Runner { child, stdin, stdout, next_id: 0 })
    }
}

#[async_trait]
impl BrowserDriver for LocalBrowserDriver {
    async fn open(&self, request: BrowserSessionRequest) -> PortResult<BrowserSession> {
        let id = format!("{}{}", self.prefix, self.next_session.fetch_add(1, Ordering::Relaxed));
        // Runner 的工作目錄是受驗目錄，所以交給它的路徑一律是絕對路徑
        let dir = std::path::absolute(request.scratch_dir.join(&id)).map_err(|e| failed(format!("無法解析暫存目錄：{e}")))?;
        let mut runner = self.spawn(&request, &dir).await?;
        let open = json!({
            "op": "open",
            "workingDir": std::path::absolute(&request.working_dir).unwrap_or_else(|_| request.working_dir.clone()),
            "tracePath": request.trace.then(|| dir.join(TRACE_FILE)),
            "videoDir": request.video.then(|| dir.join(VIDEO_DIR)),
        });
        // 開啟失敗時 runner 在這裡被丟棄，程序隨之終止
        let response = runner
            .request(open, OPEN_TIMEOUT)
            .await?
            .ok_or_else(|| failed(format!("瀏覽器在 {} 秒內沒有開啟", OPEN_TIMEOUT.as_secs())))?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(failed(message_of(&response)));
        }
        if let Some(engine) = response.get("engine").and_then(Value::as_str) {
            self.engines.lock().expect("引擎表不會中毒").insert(id.clone(), engine.to_string());
        }
        self.sessions.lock().expect("Session 表不會中毒").insert(id.clone(), Arc::new(tokio::sync::Mutex::new(runner)));
        Ok(BrowserSession(id))
    }

    async fn step(&self, session: &BrowserSession, step: BrowserStep) -> PortResult<BrowserOutcome> {
        let runner = self.session(session).ok_or_else(|| failed(format!("沒有這個瀏覽器 Session：{}", session.0)))?;
        let secret = match &step.command {
            BrowserCommand::Fill { text, .. } => Some(text.clone()),
            _ => None,
        };
        let request = json!({
            "op": "step",
            "command": command_json(&step.command),
            "timeoutMs": step.timeout.as_millis() as u64,
            "visible": step.visible,
            "mask": step.screenshot_mask,
        });
        let response = runner.lock().await.request(request, step.timeout + STEP_GRACE).await?;
        let Some(response) = response else { return Ok(BrowserOutcome::TimedOut) };
        let outcome = parse_step_response(&response)?;
        Ok(match secret {
            Some(secret) => redact_outcome(outcome, &secret),
            None => outcome,
        })
    }

    async fn close(&self, session: &BrowserSession) -> PortResult<Vec<BrowserFile>> {
        let runner = self.sessions.lock().expect("Session 表不會中毒").remove(&session.0);
        self.engines.lock().expect("引擎表不會中毒").remove(&session.0);
        let Some(runner) = runner else {
            // 這個 Runtime 開啟、已經關閉的 Session：重複關閉無害。其他 Runtime 開啟的 Session 無法確認已關閉
            return if session.0.starts_with(&self.prefix) {
                Ok(Vec::new())
            } else {
                Err(failed(format!("瀏覽器 Session「{}」不是目前的 Runtime 開啟的，無法確認已關閉", session.0)))
            };
        };
        let mut runner = runner.lock().await;
        let response = runner.request(json!({ "op": "close" }), CLOSE_TIMEOUT).await;
        // 不論關閉是否成功，Runner 程序一定結束
        let _ = tokio::time::timeout(Duration::from_secs(5), runner.child.wait()).await;
        let _ = runner.child.kill().await;
        let response = response?.ok_or_else(|| failed(format!("瀏覽器在 {} 秒內沒有關閉", CLOSE_TIMEOUT.as_secs())))?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(failed(message_of(&response)));
        }
        let mut files = Vec::new();
        for file in response.get("files").and_then(Value::as_array).into_iter().flatten() {
            let (Some(kind), Some(path)) = (file.get("kind").and_then(Value::as_str), file.get("path").and_then(Value::as_str)) else {
                continue;
            };
            let path = PathBuf::from(path);
            let bytes = tokio::fs::read(&path).await.map_err(|e| failed(format!("無法讀取 {}：{e}", path.display())))?;
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| kind.to_string());
            files.push(BrowserFile { kind: kind.to_string(), name, bytes });
        }
        Ok(files)
    }

    fn engine(&self, session: &BrowserSession) -> Option<String> {
        self.engines.lock().expect("引擎表不會中毒").get(&session.0).cloned()
    }
}

fn command_json(command: &BrowserCommand) -> Value {
    match command {
        BrowserCommand::Goto { url } => json!({ "kind": "goto", "url": url }),
        BrowserCommand::Click { selector } => json!({ "kind": "click", "selector": selector }),
        BrowserCommand::Fill { selector, text } => json!({ "kind": "fill", "selector": selector, "text": text }),
        BrowserCommand::Press { selector, key } => json!({ "kind": "press", "selector": selector, "key": key }),
        BrowserCommand::WaitFor { selector } => json!({ "kind": "waitFor", "selector": selector }),
    }
}

#[derive(Deserialize)]
struct RawPage {
    url: String,
    title: String,
    status: Option<u16>,
    text: String,
    visible: Vec<bool>,
    screenshot: Option<String>,
}

fn page_of(value: Option<&Value>) -> PortResult<Option<PageSnapshot>> {
    let Some(value) = value.filter(|v| !v.is_null()) else { return Ok(None) };
    let raw: RawPage = serde_json::from_value(value.clone()).map_err(|e| failed(format!("無法解讀瀏覽器 Runner 回報的畫面：{e}")))?;
    let screenshot = raw
        .screenshot
        .map(|png| base64::engine::general_purpose::STANDARD.decode(png))
        .transpose()
        .map_err(|e| failed(format!("無法解讀瀏覽器 Runner 回報的截圖：{e}")))?;
    Ok(Some(PageSnapshot { url: raw.url, title: raw.title, status: raw.status, text: raw.text, visible: raw.visible, screenshot }))
}

fn parse_step_response(response: &Value) -> PortResult<BrowserOutcome> {
    let text = |key: &str| response.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    match response.get("outcome").and_then(Value::as_str) {
        Some("done") => {
            let page = page_of(response.get("page"))?.ok_or_else(|| failed("瀏覽器 Runner 沒有回報畫面".to_string()))?;
            Ok(BrowserOutcome::Done { page })
        }
        Some("actionFailed") => Ok(BrowserOutcome::ActionFailed {
            error: text("error"),
            message: text("message"),
            page: page_of(response.get("page"))?,
        }),
        Some("unreachable") => Ok(BrowserOutcome::Unreachable { reason: text("reason") }),
        _ => Err(failed(message_of(response))),
    }
}

/// Runner 已去掉訊息中輸入過的文字；這裡對這一步的文字再檢查一次。
fn redact_outcome(outcome: BrowserOutcome, secret: &str) -> BrowserOutcome {
    if secret.is_empty() {
        return outcome;
    }
    match outcome {
        BrowserOutcome::ActionFailed { error, message, page } => {
            BrowserOutcome::ActionFailed { error, message: message.replace(secret, REDACTED), page }
        }
        BrowserOutcome::Unreachable { reason } => BrowserOutcome::Unreachable { reason: reason.replace(secret, REDACTED) },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_text_reaches_the_runner_only_inside_the_request_body() {
        let command = BrowserCommand::Fill { selector: "#password".into(), text: "hunter22".into() };
        assert_eq!(command_json(&command), json!({ "kind": "fill", "selector": "#password", "text": "hunter22" }));
        assert!(!format!("{command:?}").contains("hunter22"));
    }

    #[test]
    fn step_responses_map_to_outcomes() {
        let page = json!({ "url": "http://x/", "title": "T", "status": 200, "text": "hi", "visible": [true], "screenshot": "iVBORw==" });
        let done = parse_step_response(&json!({ "id": 1, "outcome": "done", "page": page })).unwrap();
        let BrowserOutcome::Done { page } = done else { panic!("{done:?}") };
        assert_eq!((page.status, page.visible, page.screenshot), (Some(200), vec![true], Some(vec![0x89, 0x50, 0x4e, 0x47])));
        let failed = parse_step_response(&json!({ "outcome": "actionFailed", "error": "TimeoutError", "message": "m", "page": null })).unwrap();
        assert_eq!(failed, BrowserOutcome::ActionFailed { error: "TimeoutError".into(), message: "m".into(), page: None });
        let unreachable = parse_step_response(&json!({ "outcome": "unreachable", "reason": "net::ERR_CONNECTION_REFUSED" })).unwrap();
        assert!(matches!(unreachable, BrowserOutcome::Unreachable { .. }));
        assert!(parse_step_response(&json!({ "ok": false, "message": "壞了" })).is_err());
    }

    #[test]
    fn the_fill_text_is_removed_from_failure_messages() {
        let outcome = BrowserOutcome::ActionFailed { error: "Error".into(), message: "cannot type hunter22 into #x".into(), page: None };
        let BrowserOutcome::ActionFailed { message, .. } = redact_outcome(outcome, "hunter22") else { panic!() };
        assert_eq!(message, "cannot type *** into #x");
    }
}
