use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::PortResult;

/// 啟動程序所需的描述。
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: Option<String>,
    /// 這次啟動的識別：Supervisor 在啟動前先持久化，ProcessControl 以它標記受管的程序，
    /// Runtime 重建後可用 `find` 找回原執行，不重複啟動（spec D-10）。
    pub command_id: String,
    /// 額外交給子程序的環境變數（例如 Agent 憑證），同名時蓋過 Runtime 自己的環境。
    /// 不納入 `command_hash`，也不會出現在 `Debug` 輸出，避免憑證進入日誌。
    pub env: Vec<(String, String)>,
    /// 啟動前從繼承的環境中移除的變數名稱（不分大小寫）；同名出現在 `env` 時仍以 `env` 的值設定。
    pub env_remove: Vec<String>,
    /// 標準輸入改由這個檔案提供；`None` 時是 NUL。
    pub stdin_file: Option<String>,
    /// 標準輸出寫入這個檔案（建立或覆寫，寫入期間其他程序可讀取）；`None` 時是 NUL。
    pub stdout_file: Option<String>,
    /// 標準錯誤寫入這個檔案（規則同 `stdout_file`）；`None` 時是 NUL。
    pub stderr_file: Option<String>,
}

impl std::fmt::Debug for SpawnSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env_names: Vec<&str> = self.env.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("SpawnSpec")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("working_dir", &self.working_dir)
            .field("command_id", &self.command_id)
            .field("env", &env_names)
            .field("env_remove", &self.env_remove)
            .field("stdin_file", &self.stdin_file)
            .field("stdout_file", &self.stdout_file)
            .field("stderr_file", &self.stderr_file)
            .finish()
    }
}

/// 啟動指令雜湊（SHA-256，十六進位小寫）：program 與每個引數以 NUL 分隔後計算。
/// 不含 `command_id` 與工作目錄，所以同一個指令永遠得到同一個雜湊。
pub fn command_hash(spec: &SpawnSpec) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(spec.program.as_bytes());
    for arg in &spec.args {
        hasher.update([0u8]);
        hasher.update(arg.as_bytes());
    }
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 程序身分 = PID + 建立時間 + 啟動指令雜湊（spec D-10），避免 PID 重用誤判。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub created_at_millis: i64,
    pub command_hash: String,
}

/// spawn 完成後的回執。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub identity: ProcessIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    Dead,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopResult {
    StoppedGracefully,
    ForceKilled,
    StillAlive,
}

#[async_trait]
pub trait ProcessControl: Send + Sync {
    /// 啟動程序（放進該 Run 專屬的 Job Object）並回傳回執；回傳之前程序就已被標記為 `spec.command_id`。
    async fn spawn(&self, spec: SpawnSpec) -> PortResult<Receipt>;
    /// 以 `spec.command_id` 找回先前啟動、仍在受管的程序；沒有（從未啟動，或已全部結束）回傳 `None`。
    /// 回傳 `Some` 同時代表 ProcessControl 之後可以對該身分 `inspect` 與 `stop`（Runtime 重建後的接續）。
    async fn find(&self, spec: &SpawnSpec) -> PortResult<Option<Receipt>>;
    async fn inspect(&self, identity: &ProcessIdentity) -> PortResult<Liveness>;
    async fn stop(&self, identity: &ProcessIdentity, grace: Duration) -> PortResult<StopResult>;
}
