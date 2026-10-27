use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

use crate::{CommandOutput, CommandRequest, PortResult};

/// 驗證 Adapter 送出的 HTTP 請求（票 45）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub timeout: Duration,
}

/// HTTP 請求的結果；`body` 是回應本文的結尾部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpOutcome {
    Responded { status: u16, body: String },
    /// 連不上（拒絕連線、DNS 失敗、TLS 失敗等）。
    Unreachable { reason: String },
    TimedOut,
}

/// 要啟動的長駐受驗服務。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRequest {
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
}

/// 已啟動服務的識別（由實作決定內容）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceHandle(pub String);

/// CLI／API 驗證 Adapter 對外的全部副作用（票 45，spec D-29）：執行命令、送 HTTP、啟動與停止受驗服務、
/// 就緒重試之間的等待、讀取受驗目錄的 Git HEAD，以及建立與移除本次驗證專屬的狀態目錄。
#[async_trait]
pub trait VerificationDriver: Send + Sync {
    /// 執行命令直到結束或逾時；無法啟動（例如找不到程式）時回傳錯誤。
    async fn run_command(&self, request: CommandRequest) -> PortResult<CommandOutput>;
    async fn send_http(&self, request: HttpRequest) -> PortResult<HttpOutcome>;
    /// 啟動服務；無法啟動時回傳錯誤。
    async fn start_service(&self, request: ServiceRequest) -> PortResult<ServiceHandle>;
    /// 停止這次驗證啟動的服務（只停它，不影響其他程序）。
    async fn stop_service(&self, handle: &ServiceHandle) -> PortResult<()>;
    /// 就緒條件兩次嘗試之間的等待。
    async fn pause(&self, duration: Duration);
    /// 目錄的 Git HEAD commit；不是 Git repo 時回傳 `None`。
    async fn revision(&self, dir: &Path) -> PortResult<Option<String>>;
    /// 建立空的狀態目錄（已存在時先清空：那是同一個驗證要求先前中斷留下的）。
    async fn prepare_state_dir(&self, dir: &Path) -> PortResult<()>;
    /// 移除狀態目錄；不存在不是錯誤。
    async fn remove_state_dir(&self, dir: &Path) -> PortResult<()>;
}
