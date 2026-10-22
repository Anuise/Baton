//! 正式的 SemanticJudge（票 46）：以 `claude -p` 做一次不給任何工具的語意判斷。

use async_trait::async_trait;
use baton_ports::{JudgeOutcome, JudgeRequest, PortError, PortResult, SemanticJudge};

#[derive(Debug, Default)]
pub struct ClaudeSemanticJudge;

#[async_trait]
impl SemanticJudge for ClaudeSemanticJudge {
    async fn judge(&self, _request: JudgeRequest) -> PortResult<JudgeOutcome> {
        Err(PortError::NotImplemented("SemanticJudge"))
    }
}
