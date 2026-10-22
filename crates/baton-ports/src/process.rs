use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::PortResult;

/// 啟動程序所需的描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: Option<String>,
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
    async fn spawn(&self, spec: SpawnSpec) -> PortResult<Receipt>;
    async fn inspect(&self, identity: &ProcessIdentity) -> PortResult<Liveness>;
    async fn stop(&self, identity: &ProcessIdentity, grace: Duration) -> PortResult<StopResult>;
}
