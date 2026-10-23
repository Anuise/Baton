//! Supervisor 的 Controller CLI 步驟（desktop-controller-cli 票 03，ADR-0014）。
//!
//! 每次處理時看所有尚未結束的 Controller Run：
//! - 剛被要求開啟（還沒有 launch intent）：先寫下 launch intent，再確認 `claude`、準備設定快照與 MCP 設定、簽發 Controller 憑證，
//!   經 Claude 環境 Port 開出終端機並寫回回執。任何一步失敗都以原因結束這個 Run（頁簽顯示原因、不留下非終態 Run）。
//! - 有 launch intent 而沒有回執：上一次開窗的結果不明（Runtime 在開窗與回執之間中斷）。以 command_id 找回宿主就補寫回執，
//!   找不到就結束這個 Run；**不重複開窗**。
//! - 有回執：使用者要求結束就停止宿主（連同它啟動的 `claude`）；否則以既有的程序存活查驗，宿主不在就結束。
//!
//! Run 結束時，以它簽發的 Controller 憑證隨之失效（憑證的有效性取決於 Run 是否尚未結束）。
//! 使用者要求換帳號重開時（票 05），結束宿主的同一個交易中以新 Profile 開新的 Run，下一次處理就開窗。

use std::path::Path;
use std::time::Duration;

use baton_api::RUN_SETTINGS_DIR;
use baton_domain::agent::{AGENT_TOKEN_ENV, RUNTIME_URL_ENV};
use baton_domain::auth::ControllerScope;
use baton_domain::controller_cli::{
    ControllerCliRun, ControllerCliStatus, OpenIds, END_CLAUDE_NOT_FOUND, END_HOST_GONE, END_LAUNCH_FAILED,
    END_LAUNCH_OUTCOME_UNKNOWN, END_SWITCHED, END_TERMINAL_FAILED, END_USER_ENDED, WINDOW_TITLE_PREFIX,
};
use baton_domain::shared_settings::{self, MANIFEST_FILE};
use baton_ports::{Capability, Liveness, RunSpec, SpawnSpec, StopResult, TerminalLaunch, ToolProbe};

use super::{identity_of, new_prefixed_id, process_record, Supervisor};

/// 使用者按「結束」時，給宿主自行結束的時間；之後終止整棵程序樹。
const CONTROLLER_CLI_STOP_GRACE: Duration = Duration::from_secs(2);

impl Supervisor {
    pub(super) async fn drive_controller_clis(&self) {
        let runs = match self.store.open_controller_cli_runs().await {
            Ok(runs) => runs,
            Err(error) => return self.note(&format!("無法讀取 Controller CLI：{error}")),
        };
        for run in runs {
            if run.launched_at.is_none() {
                self.launch_controller_cli(&run).await;
            } else if run.host.is_none() {
                self.recover_controller_cli(&run).await;
            } else {
                self.watch_controller_cli(&run).await;
            }
        }
    }

    async fn launch_controller_cli(&self, run: &ControllerCliRun) {
        if run.end_requested_at.is_some() {
            let (code, reason) = match run.switch_to_profile_id {
                Some(_) => (END_SWITCHED, "開窗之前已換帳號重開"),
                None => (END_USER_ENDED, "開窗之前已從 App 結束"),
            };
            return self.end_controller_cli(run, code, reason).await;
        }
        // 1. 先持久化 launch intent；這一步完成之前不會開任何視窗（Profile 已失效時改為結束，不開窗）。
        //    同帳號重開要接續時，一併決定原生 Session 還在不在
        let continuable = !run.continue_session || self.native_session_continuable(run).await;
        let run = match self.store.begin_controller_cli_launch(&run.id, continuable, self.ports.clock.now()).await {
            Ok(run) if run.status == ControllerCliStatus::Ended => return,
            Ok(run) => run,
            Err(error) => return self.note(&format!("Controller CLI {} 無法寫入 launch intent，不開窗：{error}", run.id)),
        };
        // 2. 確認找得到 claude（宿主找不到時視窗只會一閃而過）
        match self.ports.claude_env.probe_claude().await {
            Ok(ToolProbe::Found { .. }) => {}
            Ok(ToolProbe::NotFound) => {
                let reason = "找不到 Claude Code（claude）；請安裝並確認它在 PATH 上，再重新開啟";
                return self.end_controller_cli(&run, END_CLAUDE_NOT_FOUND, reason).await;
            }
            Err(error) => {
                let reason = format!("無法確認 Claude Code（claude）是否可用：{error}");
                return self.end_controller_cli(&run, END_CLAUDE_NOT_FOUND, &reason).await;
            }
        }
        // 3. 設定快照（開啟時固定的共同設定版本）與 MCP 設定
        let spec = match self.controller_cli_spec(&run).await {
            Ok(spec) => spec,
            Err(reason) => return self.end_controller_cli(&run, END_LAUNCH_FAILED, &reason).await,
        };
        if let Err(reason) = self.write_controller_cli_settings(&run).await {
            return self.end_controller_cli(&run, END_LAUNCH_FAILED, &reason).await;
        }
        if let Err(error) = self.ports.agent_adapter.prepare(&spec).await {
            let reason = format!("無法準備 Controller CLI 的啟動檔案：{error}");
            return self.end_controller_cli(&run, END_LAUNCH_FAILED, &reason).await;
        }
        // 4. Controller 憑證：綁定這個 Run，只經環境變數交給宿主（再由 claude 交給 MCP shim）；Run 結束即失效
        let scope = ControllerScope { workspace_id: run.workspace_id.clone(), controller_run_id: run.id.clone() };
        let token = match self.credentials.issue_controller(scope).await {
            Ok(token) => token,
            Err(error) => {
                let reason = format!("無法簽發 Controller 憑證：{error}");
                return self.end_controller_cli(&run, END_LAUNCH_FAILED, &reason).await;
            }
        };
        // 5. 開窗
        let mut spawn = self.ports.agent_adapter.controller_spawn_spec(&spec);
        let lookup = spawn.clone();
        spawn.env.push((AGENT_TOKEN_ENV.to_string(), token));
        spawn.env.push((RUNTIME_URL_ENV.to_string(), self.runtime_url.clone()));
        let launch = TerminalLaunch { spawn, title: self.controller_cli_title(&run).await };
        let identity = match self.ports.claude_env.open_controller_terminal(&launch).await {
            Ok(identity) => identity,
            // 開窗回報失敗不代表沒有開出宿主：先以 command_id 查證
            Err(error) => match self.ports.process_control.find(&lookup).await {
                Ok(Some(receipt)) => receipt.identity,
                Ok(None) => {
                    let reason = format!("無法開啟 Controller 終端機：{error}");
                    return self.end_controller_cli(&run, END_TERMINAL_FAILED, &reason).await;
                }
                Err(find_error) => {
                    return self.note(&format!(
                        "Controller CLI {} 開窗回報失敗（{error}），也無法以 command_id 查證（{find_error}），留待下次核對",
                        run.id
                    ));
                }
            },
        };
        // 6. 回執寫回；寫回之前當掉的話，下一次處理以 command_id 找回
        if let Err(error) = self.store.record_controller_cli_receipt(&run.id, process_record(&identity), self.ports.clock.now()).await {
            self.note(&format!("Controller CLI {} 的開窗回執沒有寫回，下次以 command_id 核對：{error}", run.id));
        }
    }

    /// launch intent 已寫、沒有回執：以 command_id 找回宿主就補寫回執；找不到就結束，不重複開窗。
    async fn recover_controller_cli(&self, run: &ControllerCliRun) {
        let spawn = match self.controller_cli_lookup(run).await {
            Ok(spawn) => spawn,
            Err(error) => return self.note(&format!("Controller CLI {} 暫時無法核對宿主，稍後重試：{error}", run.id)),
        };
        match self.ports.process_control.find(&spawn).await {
            Ok(Some(receipt)) => {
                self.note(&format!("Controller CLI {} 以 command_id 找回宿主 {}，補寫回執，不重複開窗", run.id, receipt.identity.pid));
                let now = self.ports.clock.now();
                if let Err(error) = self.store.record_controller_cli_receipt(&run.id, process_record(&receipt.identity), now).await {
                    self.note(&format!("Controller CLI {} 的回執沒有寫回：{error}", run.id));
                }
            }
            Ok(None) => {
                let reason = "上一次開啟時 Runtime 中斷，找不到 Controller CLI 的終端機；為避免重複開窗，這次開啟已結束，請重新開啟";
                self.end_controller_cli(run, END_LAUNCH_OUTCOME_UNKNOWN, reason).await;
            }
            Err(error) => self.note(&format!("Controller CLI {} 暫時無法核對宿主，稍後重試：{error}", run.id)),
        }
    }

    /// 已有宿主：使用者要求結束就停止它；否則查驗存活，宿主確定不在才結束（無法判定時不下結論）。
    async fn watch_controller_cli(&self, run: &ControllerCliRun) {
        let Some(host) = run.host.as_ref() else { return };
        let identity = identity_of(host);
        if run.end_requested_at.is_some() {
            // Runtime 重建後 ProcessControl 要先找回宿主所在的程序群組才能停止它
            if let Ok(spawn) = self.controller_cli_lookup(run).await {
                let _ = self.ports.process_control.find(&spawn).await;
            }
            match self.ports.process_control.stop(&identity, CONTROLLER_CLI_STOP_GRACE).await {
                Ok(StopResult::StoppedGracefully | StopResult::ForceKilled) => match run.switch_to_profile_id {
                    Some(_) => self.end_controller_cli(run, END_SWITCHED, "已換帳號重開：結束這個 Controller CLI，改用新的帳號開啟").await,
                    None => self.end_controller_cli(run, END_USER_ENDED, "已從 App 結束 Controller CLI").await,
                },
                Ok(StopResult::StillAlive) => self.note(&format!("Controller CLI {} 的宿主仍未結束，稍後重試", run.id)),
                Err(error) => self.note(&format!("Controller CLI {} 無法結束宿主，稍後重試：{error}", run.id)),
            }
            return;
        }
        if let Ok(Liveness::Dead) = self.ports.process_control.inspect(&identity).await {
            self.end_controller_cli(run, END_HOST_GONE, "Controller CLI 的終端機已關閉").await;
        }
    }

    /// 結束 Run；已要求換帳號重開時，同一個交易中以新 Profile 開新的 Run（取不到亂數時只結束，並記下原因）。
    async fn end_controller_cli(&self, run: &ControllerCliRun, code: &str, reason: &str) {
        let next = match run.switch_to_profile_id {
            Some(_) => {
                let ids = new_controller_cli_ids();
                if ids.is_none() {
                    self.note(&format!("Controller CLI {} 換帳號重開：無法取得系統亂數，只結束、不開新的", run.id));
                }
                ids
            }
            None => None,
        };
        if let Err(error) = self.store.end_controller_cli(&run.id, code, reason, None, next, self.ports.clock.now()).await {
            self.note(&format!("Controller CLI {} 的結束沒有寫入：{error}", run.id));
        }
    }

    /// Controller CLI 的 `RunSpec`：全部來自持久化的 Run（帳號、工作目錄、開啟時固定的共同設定版本），
    /// 所以 Runtime 重建後組出相同的啟動指令，能以 command_id 找回宿主。
    async fn controller_cli_spec(&self, run: &ControllerCliRun) -> Result<RunSpec, String> {
        let settings_files = match run.settings_version {
            Some(version) => shared_settings::snapshot_files(&self.settings_version_of(version).await?)
                .into_iter()
                .map(|file| file.name)
                .filter(|name| name != MANIFEST_FILE)
                .collect(),
            None => Vec::new(),
        };
        Ok(RunSpec {
            run_id: run.id.clone(),
            task_id: String::new(),
            command_id: run.command_id.clone(),
            prompt: String::new(),
            working_dir: Some(run.working_dir.clone()),
            config_dir: Some(self.config_dir_of(&run.profile_id).await?),
            settings_dir: run.settings_version.map(|_| self.controller_cli_settings_dir(run)),
            settings_files,
            model: None,
            resume_native_session_id: None,
            continue_session: run.continue_session,
        })
    }

    /// 找回宿主用的啟動規格（不含憑證；`command_hash` 與 `command_id` 與開窗時相同）。
    async fn controller_cli_lookup(&self, run: &ControllerCliRun) -> Result<SpawnSpec, String> {
        Ok(self.ports.agent_adapter.controller_spawn_spec(&self.controller_cli_spec(run).await?))
    }

    async fn settings_version_of(&self, version: i64) -> Result<shared_settings::SettingsVersion, String> {
        self.store
            .settings_version(version)
            .await
            .map_err(|e| format!("無法讀取共同設定版本 {version}：{e}"))?
            .ok_or_else(|| format!("找不到共同設定版本 {version}"))
    }

    fn controller_cli_settings_dir(&self, run: &ControllerCliRun) -> String {
        self.data_dir.join(RUN_SETTINGS_DIR).join(&run.id).to_string_lossy().into_owned()
    }

    /// 以與 Worker 相同的機制，把開啟時固定的共同設定版本寫成這個 Run 專屬的唯讀目錄。
    async fn write_controller_cli_settings(&self, run: &ControllerCliRun) -> Result<(), String> {
        let Some(version) = run.settings_version else { return Ok(()) };
        let files: Vec<(String, Vec<u8>)> =
            shared_settings::snapshot_files(&self.settings_version_of(version).await?).into_iter().map(|f| (f.name, f.bytes)).collect();
        let directory = self.controller_cli_settings_dir(run);
        self.ports
            .workspace_fs
            .write_readonly_files(Path::new(&directory), &files)
            .await
            .map_err(|error| format!("無法寫入設定快照目錄 {directory}：{error}"))
    }

    /// 同帳號重開時，要接續的原生 Session 是否可能還在：只有 Adapter 確定沒有（`NotResumable`）才改開新的 Session；
    /// 判斷不了時照原本的決定接續。
    async fn native_session_continuable(&self, run: &ControllerCliRun) -> bool {
        let Ok(config_dir) = self.config_dir_of(&run.profile_id).await else { return true };
        match self.ports.agent_adapter.continue_capability(&config_dir, &run.working_dir).await {
            Ok(Capability::NotResumable) => {
                self.note(&format!("Controller CLI {} 找不到要接續的原生 Session，改開新的 Session", run.id));
                false
            }
            _ => true,
        }
    }

    /// 視窗標題：前綴加 Workspace 名稱（讀不到時用工作目錄）。
    async fn controller_cli_title(&self, run: &ControllerCliRun) -> String {
        let name = match self.store.list_workspaces().await {
            Ok(workspaces) => workspaces.into_iter().find(|w| w.id == run.workspace_id).map(|w| w.name),
            Err(_) => None,
        };
        format!("{WINDOW_TITLE_PREFIX} - {}", name.unwrap_or_else(|| run.working_dir.clone()))
    }
}

/// 換帳號重開時新 Run 的識別（與 API 開啟使用相同格式）。
fn new_controller_cli_ids() -> Option<OpenIds> {
    Some(OpenIds { run_id: new_prefixed_id("ctl")?, command_id: new_prefixed_id("cmd")? })
}
