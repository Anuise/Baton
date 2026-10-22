//! 確定性檢查的執行（票 13）：依契約宣告的檢查經由 Port 取得事實（命令結束碼、檔案內容、成果檔案），
//! 交給 Domain 判定成檢查結果。這裡不寫入任何東西；結果由 Supervisor 以成果接受交易提交。

use std::path::{Path, PathBuf};
use std::time::Duration;

use baton_domain::verification::{
    judge, CheckResult, CommandObservation, Observation, VerificationCheck, COMMAND_TIMEOUT_MS,
};
use baton_ports::{CommandOutput, CommandRequest, Ports};
use baton_store::PendingVerification;

/// 依序執行一個 Task 的全部檢查，結果的順序與契約宣告的順序相同。
pub async fn run_checks(ports: &Ports, artifacts_root: &Path, item: &PendingVerification) -> Vec<CheckResult> {
    let mut results = Vec::with_capacity(item.pending.checks.len());
    for check in &item.pending.checks {
        let observation = observe(ports, artifacts_root, item, check).await;
        results.push(judge(check, observation));
    }
    results
}

async fn observe(ports: &Ports, artifacts_root: &Path, item: &PendingVerification, check: &VerificationCheck) -> Observation {
    match check {
        VerificationCheck::Command { program, args, .. } => {
            let request = CommandRequest {
                program: program.clone(),
                args: args.clone(),
                working_dir: PathBuf::from(&item.working_dir),
                timeout: Duration::from_millis(COMMAND_TIMEOUT_MS),
            };
            Observation::Command(match ports.command_runner.run(request).await {
                Ok(CommandOutput::Exited { code, output }) => CommandObservation::Exited { code, output },
                Ok(CommandOutput::Terminated { output }) => CommandObservation::Terminated { output },
                Ok(CommandOutput::TimedOut { output }) => CommandObservation::TimedOut { output },
                Err(error) => CommandObservation::NotStarted { reason: error.to_string() },
            })
        }
        // 路徑在契約儲存時已驗證為工作目錄內的相對路徑
        VerificationCheck::File { path, .. } => Observation::File(
            ports
                .workspace_fs
                .read_workspace_file(&Path::new(&item.working_dir).join(path))
                .await
                .map_err(|e| e.to_string()),
        ),
        VerificationCheck::Artifact { name, .. } => {
            let latest = item.pending.manifest.iter().find(|entry| &entry.name == name).cloned();
            let storage_path = latest.as_ref().and_then(|entry| {
                item.artifacts
                    .iter()
                    .find(|a| a.id == entry.artifact_id)
                    .and_then(|a| a.versions.iter().find(|v| v.version == entry.version))
                    .map(|v| v.storage_path.clone())
            });
            let content = match storage_path {
                Some(storage_path) => {
                    ports.workspace_fs.read_artifact(&artifacts_root.join(storage_path)).await.map_err(|e| e.to_string())
                }
                None => Ok(None),
            };
            Observation::Artifact { latest, content }
        }
    }
}
