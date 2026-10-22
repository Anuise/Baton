//! 驅動 `driver.mjs`（Playwright + Chromium）的用戶端：一個要求一行 JSON、一個回應一行 JSON。
//!
//! 目標元素以「定位鏈」描述，例如 `&[role("region", "工作面板"), role("button", "派工")]`：
//! 每一節在前一節找到的範圍內再找（Playwright 的 `getByRole`／`getByLabel`／`getByText`）。
//! 多個元素符合時 Playwright 會報錯，不會任意挑一個；需要時以 `nth` 指定。
//!
//! 每個操作有 15 秒上限（等待元素出現、可點擊）；失敗時測試以 Playwright 的訊息與當下的畫面文字失敗。
//! 這裡不用 `kill_on_drop`：測試 panic 而丟棄 `Browser` 時，driver 讀到 stdin 結束會自行關閉瀏覽器再結束，
//! 直接終止 driver 反而會留下 Chromium。

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

/// 一般操作的等待上限（driver 內也以同一個值當 Playwright 的 timeout）。
const ACTION_TIMEOUT: Duration = Duration::from_secs(15);
/// 啟動與關閉瀏覽器的等待上限。
const BROWSER_TIMEOUT: Duration = Duration::from_secs(60);

/// 以角色與無障礙名稱（完全相符）找元素，例如 `role("button", "派工")`。
pub fn role(role: &str, name: &str) -> Value {
    json!({ "role": role, "name": name })
}

/// 以標籤文字（完全相符）找表單欄位。
pub fn label(text: &str) -> Value {
    json!({ "label": text })
}

/// 以包含的文字找元素。
pub fn text(text: &str) -> Value {
    json!({ "text": text })
}

/// 前一節找到多個元素時取第 `index` 個（從 0 開始）。
pub fn nth(index: usize) -> Value {
    json!({ "nth": index })
}

/// 前一節找到的元素中，只留下內含這段文字的。
pub fn has_text(text: &str) -> Value {
    json!({ "hasText": text })
}

struct Driver {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

pub struct Browser {
    driver: Mutex<Option<Driver>>,
}

fn driver_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("ui_e2e").join("driver.mjs")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

impl Driver {
    async fn request(&mut self, mut request: Value, wait: Duration) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        request["id"] = json!(id);
        let mut line = serde_json::to_vec(&request).expect("要求可以編碼");
        line.push(b'\n');
        self.stdin.write_all(&line).await.expect("driver 已結束（寫入失敗）");
        self.stdin.flush().await.expect("driver 已結束（寫入失敗）");
        let stdout = &mut self.stdout;
        let read = async {
            loop {
                let text = stdout.next_line().await.expect("無法讀取 driver 的輸出").expect("driver 意外結束");
                // 不是協定的行（例如 Playwright 印出的訊息）略過
                let Ok(response) = serde_json::from_str::<Value>(&text) else { continue };
                if response["id"] == json!(id) {
                    return response;
                }
            }
        };
        tokio::time::timeout(wait, read).await.unwrap_or_else(|_| panic!("driver 在 {wait:?} 內沒有回應：{request}"))
    }
}

impl Browser {
    /// 啟動 driver 與 Chromium。找不到 Playwright 時失敗（不是略過）：只有明確要求執行 UI E2E 才會跑到這裡。
    pub async fn open() -> Self {
        let mut child = tokio::process::Command::new("node")
            .arg(driver_script())
            .current_dir(repo_root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("無法啟動 Node.js；UI E2E 需要 Node.js 與 Playwright");
        let stdin = child.stdin.take().expect("stdin 已設為 piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout 已設為 piped")).lines();
        let mut driver = Driver { child, stdin, stdout, next_id: 0 };
        let response = driver.request(json!({ "op": "open", "timeoutMs": ACTION_TIMEOUT.as_millis() as u64 }), BROWSER_TIMEOUT).await;
        if response["ok"] != true {
            let _ = driver.child.wait().await;
            panic!(
                "UI E2E 無法開啟瀏覽器：{}；請設定 BATON_PLAYWRIGHT_DIR 指向已安裝 playwright 與其瀏覽器的目錄",
                response["message"]
            );
        }
        Self { driver: Mutex::new(Some(driver)) }
    }

    async fn call(&self, request: Value) -> Value {
        let mut guard = self.driver.lock().await;
        let driver = guard.as_mut().expect("瀏覽器已關閉");
        let description = request.to_string();
        let response = driver.request(request, ACTION_TIMEOUT + Duration::from_secs(15)).await;
        if response["ok"] != true {
            panic!(
                "瀏覽器操作失敗：{description}\n原因：{}\n當下畫面：\n{}",
                response["message"].as_str().unwrap_or_default(),
                response["page"].as_str().unwrap_or_default()
            );
        }
        response
    }

    pub async fn goto(&self, url: &str) {
        self.call(json!({ "op": "goto", "url": url })).await;
    }

    pub async fn click(&self, target: &[Value]) {
        self.call(json!({ "op": "click", "target": target })).await;
    }

    pub async fn fill(&self, target: &[Value], value: &str) {
        self.call(json!({ "op": "fill", "target": target, "value": value })).await;
    }

    /// 在下拉選單中選取顯示文字為 `option` 的選項。
    pub async fn select(&self, target: &[Value], option: &str) {
        self.call(json!({ "op": "select", "target": target, "option": option })).await;
    }

    pub async fn check(&self, target: &[Value]) {
        self.call(json!({ "op": "check", "target": target })).await;
    }

    /// 等到元素出現在畫面上。
    pub async fn wait_visible(&self, target: &[Value]) {
        self.call(json!({ "op": "waitVisible", "target": target })).await;
    }

    /// 等到元素從畫面上消失（或不存在）。
    pub async fn wait_hidden(&self, target: &[Value]) {
        self.call(json!({ "op": "waitHidden", "target": target })).await;
    }

    /// 元素的可見文字（`innerText`）。
    pub async fn text(&self, target: &[Value]) -> String {
        self.call(json!({ "op": "text", "target": target })).await["value"].as_str().unwrap_or_default().to_string()
    }

    /// 目前符合的元素數量（不等待）。
    pub async fn count(&self, target: &[Value]) -> usize {
        self.call(json!({ "op": "count", "target": target })).await["value"].as_u64().unwrap_or_default() as usize
    }

    /// 關閉瀏覽器並等 driver 結束；斷言 Chromium 的程序已結束。
    pub async fn close(self) {
        let mut driver = self.driver.lock().await.take().expect("瀏覽器已關閉");
        let response = driver.request(json!({ "op": "close" }), BROWSER_TIMEOUT).await;
        assert_eq!(response["ok"], true, "無法關閉瀏覽器：{response}");
        assert_eq!(response["browserExited"], true, "Chromium 的程序在關閉後仍存在：{response}");
        drop(driver.stdin);
        let status = tokio::time::timeout(BROWSER_TIMEOUT, driver.child.wait())
            .await
            .expect("driver 在關閉瀏覽器後沒有結束")
            .expect("無法等待 driver 結束");
        assert!(status.success(), "driver 結束碼 {status}");
    }
}
