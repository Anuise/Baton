use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use crate::PortResult;

/// 驗證命令的執行要求（票 13）：不經過 shell，在工作目錄中執行，超過逾時就終止。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    pub timeout: Duration,
}

/// 命令執行的結果；`output` 是標準輸出與標準錯誤合併後的結尾部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutput {
    Exited { code: i32, output: String },
    /// 程序被終止而沒有結束碼。
    Terminated { output: String },
    TimedOut { output: String },
}

#[async_trait]
pub trait CommandRunner: Send + Sync {
    /// 執行命令直到結束或逾時；無法啟動（例如找不到程式）時回傳錯誤。
    async fn run(&self, request: CommandRequest) -> PortResult<CommandOutput>;
}
