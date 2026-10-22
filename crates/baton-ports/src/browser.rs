use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use crate::PortResult;

/// 開啟一個隔離的瀏覽器 Session（票 46）：每條旅程一個，不共用 cookie 與登入狀態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserSessionRequest {
    /// 受驗的目錄（正式實作從這裡解析專案自己的 Playwright）。
    pub working_dir: PathBuf,
    /// 本次驗證專屬的暫存目錄（trace、錄影等檔案先寫在這裡；清理時整個移除）。
    pub scratch_dir: PathBuf,
    /// 錄製 trace。
    pub trace: bool,
    /// 錄影。
    pub video: bool,
}

/// 已開啟 Session 的識別（由實作決定內容）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BrowserSession(pub String);

/// 交給瀏覽器執行的操作；`Fill` 的文字可能是已解析的秘密值（只在 Runner 內使用，不寫入任何紀錄）。
#[derive(Clone, PartialEq, Eq)]
pub enum BrowserCommand {
    Goto { url: String },
    Click { selector: String },
    Fill { selector: String, text: String },
    Press { selector: String, key: String },
    WaitFor { selector: String },
}

impl std::fmt::Debug for BrowserCommand {
    /// 不輸出 `Fill` 的文字：那可能是秘密。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrowserCommand::Goto { url } => write!(f, "Goto({url})"),
            BrowserCommand::Click { selector } => write!(f, "Click({selector})"),
            BrowserCommand::Fill { selector, .. } => write!(f, "Fill({selector}, …)"),
            BrowserCommand::Press { selector, key } => write!(f, "Press({selector}, {key})"),
            BrowserCommand::WaitFor { selector } => write!(f, "WaitFor({selector})"),
        }
    }
}

/// 一個步驟：執行操作，再觀察當前畫面。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserStep {
    pub command: BrowserCommand,
    pub timeout: Duration,
    /// 操作之後要檢查是否可見的 selector（順序與 `PageSnapshot::visible` 對應）。
    pub visible: Vec<String>,
    /// 操作之後截圖；截圖時遮住這些 selector。`None` 表示不截圖。
    pub screenshot_mask: Option<Vec<String>>,
}

/// 操作之後的當前畫面。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSnapshot {
    pub url: String,
    pub title: String,
    /// 最近一次導覽的主要回應狀態碼（沒有導覽過時為 `None`）。
    pub status: Option<u16>,
    /// 畫面上可見的文字。
    pub text: String,
    /// 依 `BrowserStep::visible` 的順序。
    pub visible: Vec<bool>,
    /// PNG 截圖（有要求時）。
    pub screenshot: Option<Vec<u8>>,
}

/// 瀏覽器步驟的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserOutcome {
    Done { page: PageSnapshot },
    /// 操作本身沒有完成（例如找不到元素）；`error` 是引擎的錯誤名稱（例如 `TimeoutError`）。
    ActionFailed { error: String, message: String, page: Option<PageSnapshot> },
    /// 連不上受驗的網址。
    Unreachable { reason: String },
    TimedOut,
}

/// Session 結束時留下的檔案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserFile {
    /// `trace` 或 `video`。
    pub kind: String,
    /// 檔名（含副檔名）。
    pub name: String,
    pub bytes: Vec<u8>,
}

/// web 驗證 Adapter 的瀏覽器 Runner（票 46，spec D-29）：以 Playwright 為基礎執行操作並回報當前畫面。
/// 判定不在這裡做；Runner 只執行與觀察。
#[async_trait]
pub trait BrowserDriver: Send + Sync {
    /// 開啟 Session；瀏覽器或引擎無法使用時回傳錯誤。
    async fn open(&self, request: BrowserSessionRequest) -> PortResult<BrowserSession>;
    async fn step(&self, session: &BrowserSession, step: BrowserStep) -> PortResult<BrowserOutcome>;
    /// 關閉 Session，回傳 trace、錄影等檔案。
    async fn close(&self, session: &BrowserSession) -> PortResult<Vec<BrowserFile>>;
}

/// 語意 judge 使用的帳號（Profile 的設定目錄）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeAccount {
    pub profile_id: String,
    pub config_dir: PathBuf,
}

/// 語意 judge 的輸入：只有當前畫面與 Profile 中固定的預期，不含任何行動者的敘述，也不含秘密。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeRequest {
    pub account: JudgeAccount,
    /// Profile 寫定的預期。
    pub expectation: String,
    /// 測試 persona 的說明（必要的領域背景；沒有 persona 時為 `None`）。
    pub persona: Option<String>,
    pub page_url: String,
    pub page_title: String,
    /// 畫面上可見的文字（已遮蔽秘密）。
    pub page_text: String,
}

/// 語意 judge 的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JudgeOutcome {
    Decided { satisfied: bool, rationale: String },
    /// 模型限流：不是產品的問題。
    RateLimited { reset_at_millis: Option<i64> },
    /// 模型無法使用或回應無法解讀。
    Unavailable { reason: String },
}

/// 受限的語意 judge（票 46）：一次模型呼叫，不給任何工具。
#[async_trait]
pub trait SemanticJudge: Send + Sync {
    async fn judge(&self, request: JudgeRequest) -> PortResult<JudgeOutcome>;
}

/// 秘密參照的解析（票 46）：Profile 與資料庫只保存參照，值只在 Runner 執行操作的當下解析。
#[async_trait]
pub trait SecretResolver: Send + Sync {
    /// 解析參照（例如 `env:SHOP_PASSWORD`）；不存在時回傳錯誤（錯誤訊息不得含秘密值）。
    async fn resolve(&self, reference: &str) -> PortResult<String>;
}
