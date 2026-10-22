//! Supervisor：launch intent → 啟動 → 回執 → 就緒 → 停止；程序身分核對（spec 「程序與模組架構」）。
//!
//! 每次 `tick` 依序做五件事，而且每一件都是冪等的、只信任資料庫中持久化的狀態：
//! 1. **啟動**派工 Outbox 裡的 Run：先在交易中持久化 launch intent 與 command_id，才在交易外 spawn，
//!    再把回執寫回。Runtime 在中間任何一點當掉，下一次 tick 都用 command_id 找回原程序，**絕不重新啟動**；
//!    找不到程序時把 Run 標為 interrupted，交給使用者重新派工，而不是猜測「應該沒啟動」。
//! 2. **收事件**：接上 Adapter 的事件串流，heartbeat 續約 lease，結束事件只讓 Run 結束，不把 Task 標為 completed。
//! 3. **取消**：對要求取消的 Run 停止整個程序樹；確認停止之後才完成取消、釋放寫入所有權。
//! 4. **核對 lease**：lease 過期只是「該去看看」：向 ProcessControl 查證，確認死亡才中斷 Run，存活就續約，不確定就維持原狀。
//! 5. **排程**：前面幾步可能釋放了帳號與全域名額，於是讓佇列中等待的 Task 依優先序逐一嘗試派出（票 19）；
//!    派出的 Run 在同一次 `tick` 就啟動。
//!
//! 正式執行時由背景迴圈呼叫 `tick`（有新的派工或取消時會被喚醒）；測試 harness 不啟動背景迴圈，
//! 由測試在想要的時刻呼叫 `tick`，搭配 fake Clock 就能完全決定性地重現任何時序。
//!
//! 另有兩件生命週期的工作（票 25）：
//! - **完全退出**（`full_exit`）：停止意圖已持久化後，先鏡像到生命週期檔，再要求每個 Run 在寬限期內停止、
//!   逾時終止程序樹並確認（spec D-11，總上限 40 秒）；無法確認停止的記為停止異常。期間握住 tick 的鎖，不會有任何啟動。
//! - **重新開啟**：Runtime 啟動時若停止意圖仍生效，第一次 `tick` 先核對（找回啟動中的程序、查證上次沒能確認停止的程序），
//!   再解除停止意圖，依各 Workspace 的「完全退出後自動續跑」設定續跑或等「繼續工作」。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use baton_api::LogSink;
use baton_domain::agent::{AGENT_TOKEN_ENV, RUNTIME_URL_ENV};
use baton_domain::auth::AgentScope;
use baton_domain::lifecycle::{
    ExitReport, ExitTiming, LifecycleFile, LifecyclePhase, RunStopOutcome, RunStopRecord, StopAnomaly,
    RUN_GONE_AFTER_EXIT_REASON,
};
use baton_domain::profile::ProfileStatus;
use baton_domain::run::{ProcessObservation, ProcessRecord, Run, RunCommand, RunOutcome, RunStatus, RunView};
use baton_ports::{
    observe_quota, AdapterEvent, AgentHandle, Liveness, Ports, ProcessIdentity, RunSpec, SpawnSpec, StopResult,
};
use baton_store::{ExecutingRun, PendingLaunch, Store};
use futures::stream::{BoxStream, FuturesUnordered};
use futures::{FutureExt, StreamExt};
use tokio::sync::{watch, Mutex, Notify};
use tokio::task::JoinHandle;

use crate::CredentialIssuer;

/// 受控停止的預設寬限期（spec D-11）；逾時由 ProcessControl 以 Job Object 強制終止整個程序樹。
pub const DEFAULT_STOP_GRACE: Duration = Duration::from_secs(30);

/// 背景迴圈沒有被喚醒時，最久多久檢查一次。
const BACKGROUND_INTERVAL: Duration = Duration::from_secs(1);

pub struct Supervisor {
    store: Store,
    ports: Ports,
    log: LogSink,
    /// Run 准入時簽發 Agent 憑證。
    credentials: CredentialIssuer,
    /// 交給 MCP shim 的 Runtime 位址（loopback IP 字面值加實際埠）。
    runtime_url: String,
    /// 要求程序停止時給它的寬限期。
    stop_grace: Duration,
    /// 同一時間只有一個 tick 在進行（背景迴圈與測試呼叫不會交錯）。
    tick_lock: Mutex<()>,
    /// 已接上的事件串流；Runtime 重建後是空的，下一次 tick 會重新接上。
    streams: Mutex<HashMap<String, BoxStream<'static, AdapterEvent>>>,
    /// 這個 Runtime 實例已經向 ProcessControl 「找回」過（取得管理權）的 Run。
    adopted: Mutex<HashSet<String>>,
    /// 完全退出的時間參數與生命週期檔所在的資料目錄。
    exit_timing: ExitTiming,
    data_dir: PathBuf,
    /// 啟動時停止意圖仍生效：記下當時的生命週期版本，第一次 tick 核對完後重新開啟（版本變了就不重新開啟）。
    reopen_from: Mutex<Option<i64>>,
}

/// Supervisor 的生命週期參數（票 25）。
pub struct LifecycleSettings {
    pub exit_timing: ExitTiming,
    pub data_dir: PathBuf,
    /// Runtime 啟動時看到的停止意圖版本；沒有停止意圖為 `None`。
    pub reopen_from: Option<i64>,
}

impl Supervisor {
    pub fn new(
        store: Store,
        ports: Ports,
        log: LogSink,
        credentials: CredentialIssuer,
        runtime_url: String,
        stop_grace: Duration,
        lifecycle: LifecycleSettings,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            ports,
            log,
            credentials,
            runtime_url,
            stop_grace,
            tick_lock: Mutex::new(()),
            streams: Mutex::new(HashMap::new()),
            adopted: Mutex::new(HashSet::new()),
            exit_timing: lifecycle.exit_timing,
            data_dir: lifecycle.data_dir,
            reopen_from: Mutex::new(lifecycle.reopen_from),
        })
    }

    /// 背景迴圈：有人喚醒或每秒一次就 `tick`，Runtime 開始停止時結束。
    pub fn spawn_background(self: &Arc<Self>, wake: Arc<Notify>, mut stop: watch::Receiver<bool>) -> JoinHandle<()> {
        let supervisor = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.wait_for(|stopping| *stopping) => break,
                    _ = wake.notified() => {}
                    _ = tokio::time::sleep(BACKGROUND_INTERVAL) => {}
                }
                supervisor.tick().await;
            }
        })
    }

    /// 處理目前所有待辦；每個步驟的失敗只記日誌，下一次 tick 會自然重試。
    pub async fn tick(&self) {
        let _serial = self.tick_lock.lock().await;
        self.reopen_if_pending().await;
        self.launch_pending().await;
        self.pump_events().await;
        self.process_cancellations().await;
        self.check_leases().await;
        if self.schedule_queue().await {
            self.launch_pending().await;
        }
    }

    fn note(&self, message: &str) {
        (self.log)(&format!("Supervisor：{message}"));
    }

    /// 套用一個 Run 命令；被 Domain 拒絕（例如已經被別的路徑結束）只記日誌，下一次 tick 以最新狀態重新判斷。
    async fn apply(&self, run: &Run, command: RunCommand) {
        if let Err(error) = self.store.apply_run_command(run.workspace_id.clone(), command, self.ports.clock.now()).await {
            self.note(&format!("Run {} 的命令沒有套用：{error}", run.id));
        }
    }

    // ------------------------------------------------------------ 生命週期（票 25）

    /// 啟動時停止意圖仍生效：先核對（找回啟動中的程序、查證上次退出沒能確認停止的程序），再重新開啟。
    async fn reopen_if_pending(&self) {
        let Some(expected) = *self.reopen_from.lock().await else { return };
        self.launch_pending().await;
        self.reconcile_after_exit().await;
        match self.store.reopen_after_exit(expected, self.ports.clock.now()).await {
            Ok(reopened) => {
                *self.reopen_from.lock().await = None;
                if reopened {
                    self.note("重新開啟：已核對程序並解除停止意圖，依各 Workspace 的設定續跑或等待「繼續工作」");
                    self.mirror_lifecycle_file().await;
                }
            }
            Err(error) => self.note(&format!("重新開啟失敗，下一次處理再試：{error}")),
        }
    }

    /// 上次完全退出時沒能確認停止的程序：重新查證，確定已經不在才結束 Run（結果不明，不自動重派）；
    /// 仍存活或無法判定的維持原狀，不會因為續跑而啟動第二個 Worker。
    async fn reconcile_after_exit(&self) {
        let executing = match self.store.executing_runs().await {
            Ok(executing) => executing,
            Err(error) => return self.note(&format!("無法讀取執行中的 Run：{error}")),
        };
        for item in executing {
            let Some(process) = item.view.run.process.as_ref() else { continue };
            self.ensure_managed(&item).await;
            if let Ok(Liveness::Dead) = self.ports.process_control.inspect(&identity_of(process)).await {
                let reason = RUN_GONE_AFTER_EXIT_REASON.to_string();
                self.apply(&item.view.run, RunCommand::Interrupt { run_id: item.view.run.id.clone(), reason }).await;
            }
        }
    }

    /// 把資料庫中的停止意圖鏡像到生命週期檔（spec D-25）；失敗時回傳原因（也記日誌）。
    async fn mirror_lifecycle_file(&self) -> Option<String> {
        let problem = match self.store.lifecycle_state().await {
            Err(error) => Some(format!("無法讀取生命週期狀態：{error}")),
            Ok(state) => {
                let file = LifecycleFile::mirror(&state, self.ports.clock.now());
                let data_dir = self.data_dir.clone();
                match tokio::task::spawn_blocking(move || baton_store::write_lifecycle_file(&data_dir, &file)).await {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(format!("無法寫入生命週期檔：{error}")),
                    Err(error) => Some(format!("無法寫入生命週期檔：{error}")),
                }
            }
        };
        if let Some(message) = &problem {
            self.note(message);
        }
        problem
    }

    /// 完全退出的停止流程（停止意圖已由 API 持久化在資料庫）：鏡像到生命週期檔，再停下全部尚未結束的 Run。
    /// 要求每個 Run 在寬限期內保存並停止，逾時由 ProcessControl 終止整個程序樹並確認；到總上限仍沒有結果的記為停止異常。
    /// 握住 tick 的鎖直到結束，所以這段期間不會啟動任何程序。回傳最終的停止報告。
    pub async fn full_exit(&self) -> Option<ExitReport> {
        let _serial = self.tick_lock.lock().await;
        // 這個實例自己要退出了，不再做啟動時的重新開啟
        *self.reopen_from.lock().await = None;
        let state = match self.store.lifecycle_state().await {
            Ok(state) => state,
            Err(error) => {
                self.note(&format!("完全退出：無法讀取停止意圖：{error}"));
                return None;
            }
        };
        if state.phase != LifecyclePhase::Stopping {
            return state.last_exit;
        }
        let mut report = state.last_exit.clone()?;
        if let Some(problem) = self.mirror_lifecycle_file().await {
            report.notes.push(problem);
        }

        let targets = match self.store.runs_to_stop_for_exit().await {
            Ok(targets) => targets,
            Err(error) => {
                report.notes.push(format!("無法讀取尚未結束的 Run：{error}"));
                Vec::new()
            }
        };
        for item in &targets {
            report.record(RunStopRecord::pending(&item.run.id, &item.run.workspace_id, &item.run.task_id));
        }
        self.save_progress(&report).await;

        let total = Duration::from_millis(self.exit_timing.total_ms().max(0) as u64);
        let grace = Duration::from_millis(self.exit_timing.stop_grace_ms.max(0) as u64);
        let deadline = tokio::time::Instant::now() + total;
        let mut stopping = FuturesUnordered::new();
        for item in targets {
            match self.process_of(&item).await {
                Ok(Some(identity)) => {
                    let process_control = self.ports.process_control.clone();
                    stopping.push(async move {
                        let result = process_control.stop(&identity, grace).await;
                        (item, result)
                    });
                }
                // 沒有任何程序（還沒啟動，或啟動結果不明且找不到程序）：直接停下
                Ok(None) => {
                    let record = self.finish_stopped(&item, RunStopOutcome::NotStarted).await;
                    report.record(record);
                    self.save_progress(&report).await;
                }
                Err(message) => {
                    report.record(anomaly(&item, StopAnomaly::StopFailed, message));
                    self.save_progress(&report).await;
                }
            }
        }

        // 到總上限仍沒有結果就不再等待（逾時的那些在下面記為停止異常）
        while let Ok(Some((item, result))) = tokio::time::timeout_at(deadline, stopping.next()).await {
            let record = match result {
                Ok(StopResult::StoppedGracefully) => self.finish_stopped(&item, RunStopOutcome::StoppedGracefully).await,
                Ok(StopResult::ForceKilled) => self.finish_stopped(&item, RunStopOutcome::ForceKilled).await,
                Ok(StopResult::StillAlive) => anomaly(
                    &item,
                    StopAnomaly::StillAlive,
                    "終止程序樹之後仍有程序存活，無法確認停止；Run 保留寫入所有權".to_string(),
                ),
                Err(error) => anomaly(&item, StopAnomaly::StopFailed, format!("停止程序失敗：{error}")),
            };
            report.record(record);
            self.save_progress(&report).await;
        }
        let unconfirmed: Vec<RunStopRecord> = report
            .runs
            .iter()
            .filter(|r| r.outcome == RunStopOutcome::Pending)
            .map(|r| RunStopRecord {
                outcome: RunStopOutcome::Anomaly,
                anomaly: Some(StopAnomaly::StopUnconfirmed),
                message: Some(format!("到 {} 秒上限仍無法確認程序停止；Run 保留寫入所有權", total.as_secs_f64())),
                ..r.clone()
            })
            .collect();
        for record in unconfirmed {
            report.record(record);
        }

        let anomalies = report.anomalies().count();
        match self.store.complete_exit(report, self.ports.clock.now()).await {
            Ok(state) => {
                self.note(&format!("完全退出：停止流程結束，停止異常 {anomalies} 個"));
                state.last_exit
            }
            Err(error) => {
                self.note(&format!("完全退出：無法寫入停止結果：{error}"));
                None
            }
        }
    }

    async fn save_progress(&self, report: &ExitReport) {
        if let Err(error) = self.store.record_exit_progress(report.clone(), self.ports.clock.now()).await {
            self.note(&format!("完全退出：無法寫入停止進度：{error}"));
        }
    }

    /// 要停下的 Run 的程序身分：還沒啟動的沒有程序；啟動中（回執未寫回）的以 command_id 找回。
    async fn process_of(&self, item: &PendingLaunch) -> Result<Option<ProcessIdentity>, String> {
        if let Some(process) = item.run.process.as_ref() {
            let executing = ExecutingRun { view: RunView::new(item.run.clone(), None, &[]), payload: item.payload.clone() };
            self.ensure_managed(&executing).await;
            return Ok(Some(identity_of(process)));
        }
        let Some(command_id) = item.run.command_id.clone() else { return Ok(None) };
        let spec = self.ports.agent_adapter.spawn_spec(&self.run_spec(item, &command_id));
        match self.ports.process_control.find(&spec).await {
            Ok(Some(receipt)) => {
                self.adopted.lock().await.insert(item.run.id.clone());
                Ok(Some(receipt.identity))
            }
            Ok(None) => Ok(None),
            Err(error) => Err(format!("無法以 command_id 找回啟動中的程序：{error}")),
        }
    }

    /// 程序確定已停止（或從未存在）：結束 Run、釋放寫入所有權，Task 退回 ready 並重新排隊。
    async fn finish_stopped(&self, item: &PendingLaunch, outcome: RunStopOutcome) -> RunStopRecord {
        self.streams.lock().await.remove(&item.run.id);
        let mut record =
            RunStopRecord { outcome, ..RunStopRecord::pending(&item.run.id, &item.run.workspace_id, &item.run.task_id) };
        if let Err(error) = self.store.stop_run_for_exit(&item.run.workspace_id, &item.run.id, self.ports.clock.now()).await
        {
            record.message = Some(format!("程序已停止，但無法寫入 Run 的結束：{error}"));
        }
        record
    }

    // ------------------------------------------------------------ 排程

    /// 讓佇列中等待的 Task 依序嘗試派出；回傳這次有沒有派出新的 Run。
    async fn schedule_queue(&self) -> bool {
        match self.store.has_dispatch_requests().await {
            Ok(true) => {}
            Ok(false) => return false,
            Err(error) => {
                self.note(&format!("無法讀取派工佇列：{error}"));
                return false;
            }
        }
        let profiles = match self.store.list_profiles().await {
            Ok(profiles) => profiles,
            Err(error) => {
                self.note(&format!("無法讀取帳號 Profile：{error}"));
                return false;
            }
        };
        let active: Vec<String> =
            profiles.iter().filter(|p| p.status == ProfileStatus::Active).map(|p| p.id.clone()).collect();
        let quota = observe_quota(&*self.ports.quota_observer, &active).await;
        match self.store.schedule(None, quota, self.ports.clock.now(), new_run_id).await {
            Ok(outcome) => {
                for started in &outcome.started {
                    self.note(&format!("Task {} 取得名額，以帳號 {} 派出 Run {}", started.task_id, started.profile_id, started.run_id));
                }
                !outcome.started.is_empty()
            }
            Err(error) => {
                self.note(&format!("排程失敗：{error}"));
                false
            }
        }
    }

    // ------------------------------------------------------------ 啟動

    async fn launch_pending(&self) {
        let pending = match self.store.pending_launches().await {
            Ok(pending) => pending,
            Err(error) => return self.note(&format!("無法讀取派工 Outbox：{error}")),
        };
        for item in pending {
            match item.run.status {
                RunStatus::Queued => self.launch(item).await,
                RunStatus::Starting => self.recover_launch(item).await,
                // 其他狀態的 Outbox 項目都已經在轉換時一併標為處理完畢，不會出現在這裡
                _ => {}
            }
        }
    }

    fn run_spec(&self, item: &PendingLaunch, command_id: &str) -> RunSpec {
        run_spec_of(&item.run.id, &item.payload.task_id, command_id, &item.payload.prompt, &item.payload.working_dir)
    }

    async fn launch(&self, item: PendingLaunch) {
        let Some(command_id) = new_command_id() else {
            return self.note(&format!("Run {} 無法取得系統亂數，不啟動程序", item.run.id));
        };
        // 1. 先持久化 launch intent 與 command_id；這一步完成之前不會有任何程序被啟動
        let begin = RunCommand::BeginLaunch { run_id: item.run.id.clone(), command_id: command_id.clone() };
        if let Err(error) = self.store.apply_run_command(item.run.workspace_id.clone(), begin, self.ports.clock.now()).await {
            return self.note(&format!("Run {} 無法寫入 launch intent，不啟動程序：{error}", item.run.id));
        }
        // 2. Run 准入：簽發綁定這個 Run 與世代的 Agent 憑證，只經環境變數交給 MCP shim。
        //    憑證不進 Prompt、指令列、設定檔或日誌；啟動前失敗就不啟動（下一次 tick 會把 Run 標為 interrupted）
        let scope = AgentScope {
            workspace_id: item.run.workspace_id.clone(),
            run_id: item.run.id.clone(),
            generation: item.run.generation,
        };
        let token = match self.credentials.issue_agent(scope).await {
            Ok(token) => token,
            Err(error) => return self.note(&format!("Run {} 無法簽發 Agent 憑證，不啟動程序：{error}", item.run.id)),
        };
        // 3. 交易外啟動程序
        let mut spec = self.ports.agent_adapter.spawn_spec(&self.run_spec(&item, &command_id));
        spec.env.push((AGENT_TOKEN_ENV.to_string(), token));
        spec.env.push((RUNTIME_URL_ENV.to_string(), self.runtime_url.clone()));
        match self.ports.process_control.spawn(spec).await {
            // 4. 回執寫回；寫回之前當掉的話，下一次 tick 以 command_id 找回
            Ok(receipt) => {
                let command = RunCommand::RecordReceipt {
                    run_id: item.run.id.clone(),
                    process: process_record(&receipt.identity),
                };
                self.apply(&item.run, command).await;
                self.adopted.lock().await.insert(item.run.id.clone());
            }
            Err(error) => {
                let command = RunCommand::LaunchFailed { run_id: item.run.id.clone(), reason: error.to_string() };
                self.apply(&item.run, command).await;
            }
        }
    }

    /// Run 停在 `starting`：上一次啟動被中斷，結果不明。以 command_id 找回原程序，找到就補寫回執；
    /// 找不到就中斷這個 Run，不自動重新啟動（結果不明的外部操作不重送）。
    async fn recover_launch(&self, item: PendingLaunch) {
        let Some(command_id) = item.run.command_id.clone() else { return };
        let spec = self.ports.agent_adapter.spawn_spec(&self.run_spec(&item, &command_id));
        match self.ports.process_control.find(&spec).await {
            Ok(Some(receipt)) => {
                self.note(&format!("Run {} 以 command_id 找回原程序 {}，補寫回執，不重複啟動", item.run.id, receipt.identity.pid));
                let command =
                    RunCommand::RecordReceipt { run_id: item.run.id.clone(), process: process_record(&receipt.identity) };
                self.apply(&item.run, command).await;
                self.adopted.lock().await.insert(item.run.id.clone());
            }
            Ok(None) => {
                let reason = "launch_outcome_unknown：找不到 command_id 對應的程序，不自動重新啟動".to_string();
                self.apply(&item.run, RunCommand::Interrupt { run_id: item.run.id.clone(), reason }).await;
            }
            Err(error) => self.note(&format!("Run {} 暫時無法核對程序，稍後重試：{error}", item.run.id)),
        }
    }

    // ------------------------------------------------------------ 事件

    async fn pump_events(&self) {
        let executing = match self.store.executing_runs().await {
            Ok(executing) => executing,
            Err(error) => return self.note(&format!("無法讀取執行中的 Run：{error}")),
        };
        for item in executing.into_iter().filter(|e| e.view.run.status == RunStatus::Running) {
            let run_id = item.view.run.id.clone();
            let existing = self.streams.lock().await.remove(&run_id);
            let mut stream = match existing {
                Some(stream) => stream,
                None => match self.attach(&item).await {
                    Some(stream) => stream,
                    None => continue,
                },
            };
            let mut finished = false;
            // 只處理目前已經到達的事件，不等待；之後的事件由下一次 tick 處理
            while let Some(next) = stream.next().now_or_never() {
                let Some(event) = next else {
                    finished = true; // 串流自行結束
                    break;
                };
                match event {
                    AdapterEvent::Heartbeat => {
                        let command = RunCommand::Heartbeat { run_id: run_id.clone(), generation: item.view.run.generation };
                        self.apply(&item.view.run, command).await;
                    }
                    AdapterEvent::Exited { success } => {
                        // 程序結束只產生 Run 結束事件；Task 是否完成由後續的驗證決定
                        let outcome = if success {
                            RunOutcome::Succeeded
                        } else {
                            RunOutcome::Failed { reason: "程序以失敗結束".into() }
                        };
                        let command =
                            RunCommand::Finish { run_id: run_id.clone(), generation: item.view.run.generation, outcome };
                        self.apply(&item.view.run, command).await;
                        finished = true;
                        break;
                    }
                    // 原生 Session、訊息與限流由後續的票處理
                    AdapterEvent::Started { .. } | AdapterEvent::Message { .. } | AdapterEvent::RateLimited { .. } => {}
                }
            }
            if !finished {
                self.streams.lock().await.insert(run_id, stream);
            }
        }
    }

    /// 接上（或在 Runtime 重建後重新接上）一個 Run 的事件串流。
    async fn attach(&self, item: &ExecutingRun) -> Option<BoxStream<'static, AdapterEvent>> {
        let run = &item.view.run;
        let command_id = run.command_id.clone()?;
        let spec = run_spec_of(&run.id, &item.payload.task_id, &command_id, &item.payload.prompt, &item.payload.working_dir);
        match self.ports.agent_adapter.launch(spec).await {
            Ok(handle) => Some(self.ports.agent_adapter.events(&AgentHandle(handle.0))),
            Err(error) => {
                self.note(&format!("Run {} 無法接上事件串流：{error}", run.id));
                None
            }
        }
    }

    // ------------------------------------------------------------ 取消

    async fn process_cancellations(&self) {
        let executing = match self.store.executing_runs().await {
            Ok(executing) => executing,
            Err(error) => return self.note(&format!("無法讀取執行中的 Run：{error}")),
        };
        for item in executing {
            let run = &item.view.run;
            let (true, Some(process)) = (run.cancel_requested_at.is_some(), run.process.as_ref()) else { continue };
            self.ensure_managed(&item).await;
            let identity = identity_of(process);
            match self.ports.process_control.stop(&identity, self.stop_grace).await {
                Ok(StopResult::StoppedGracefully | StopResult::ForceKilled) => {
                    self.streams.lock().await.remove(&run.id);
                    self.apply(run, RunCommand::ConfirmCancelled { run_id: run.id.clone() }).await;
                }
                // 程序還在：保留寫入所有權，下一次 tick 再試；這段期間不會有第二個 writer
                Ok(StopResult::StillAlive) => self.note(&format!("Run {} 的程序沒有停止，保留寫入所有權並稍後重試", run.id)),
                Err(error) => self.note(&format!("Run {} 停止程序失敗：{error}", run.id)),
            }
        }
    }

    // ------------------------------------------------------------ lease

    async fn check_leases(&self) {
        let executing = match self.store.executing_runs().await {
            Ok(executing) => executing,
            Err(error) => return self.note(&format!("無法讀取執行中的 Run：{error}")),
        };
        let now = self.ports.clock.now();
        for item in executing {
            let expired = item.view.lease.as_ref().is_some_and(|l| l.is_held() && l.is_expired(now));
            let Some(process) = item.view.run.process.as_ref().filter(|_| expired) else { continue };
            self.ensure_managed(&item).await;
            // 過期只觸發核對：向 ProcessControl 查證，不確定就不下結論
            let observed = match self.ports.process_control.inspect(&identity_of(process)).await {
                Ok(Liveness::Alive) => ProcessObservation::Alive,
                Ok(Liveness::Dead) => ProcessObservation::Dead,
                Ok(Liveness::Unknown) => ProcessObservation::Unknown,
                Err(error) => {
                    self.note(&format!("Run {} 無法核對程序：{error}", item.view.run.id));
                    ProcessObservation::Unknown
                }
            };
            self.apply(&item.view.run, RunCommand::ReconcileLease { run_id: item.view.run.id.clone(), observed }).await;
        }
    }

    /// Runtime 重建後，ProcessControl 需要先「找回」程序才能 inspect／stop 它；每個 Run 每個 Runtime 實例只做一次。
    async fn ensure_managed(&self, item: &ExecutingRun) {
        let run = &item.view.run;
        if self.adopted.lock().await.contains(&run.id) {
            return;
        }
        let Some(command_id) = run.command_id.clone() else { return };
        let spec: SpawnSpec = self.ports.agent_adapter.spawn_spec(&run_spec_of(
            &run.id,
            &item.payload.task_id,
            &command_id,
            &item.payload.prompt,
            &item.payload.working_dir,
        ));
        if self.ports.process_control.find(&spec).await.is_ok() {
            self.adopted.lock().await.insert(run.id.clone());
        }
    }
}

fn anomaly(item: &PendingLaunch, kind: StopAnomaly, message: String) -> RunStopRecord {
    RunStopRecord {
        outcome: RunStopOutcome::Anomaly,
        anomaly: Some(kind),
        message: Some(message),
        ..RunStopRecord::pending(&item.run.id, &item.run.workspace_id, &item.run.task_id)
    }
}

fn run_spec_of(run_id: &str, task_id: &str, command_id: &str, prompt: &str, working_dir: &str) -> RunSpec {
    RunSpec {
        run_id: run_id.to_string(),
        task_id: task_id.to_string(),
        command_id: command_id.to_string(),
        prompt: prompt.to_string(),
        working_dir: Some(working_dir.to_string()),
    }
}

fn process_record(identity: &ProcessIdentity) -> ProcessRecord {
    ProcessRecord {
        pid: identity.pid,
        created_at_millis: identity.created_at_millis,
        command_hash: identity.command_hash.clone(),
    }
}

fn identity_of(process: &ProcessRecord) -> ProcessIdentity {
    ProcessIdentity {
        pid: process.pid,
        created_at_millis: process.created_at_millis,
        command_hash: process.command_hash.clone(),
    }
}

/// 每次啟動的識別：96 位元隨機值；取不到系統亂數時回傳 `None`。
fn new_command_id() -> Option<String> {
    new_prefixed_id("cmd")
}

/// 排程派出的 Run 的識別（與 API 派工使用相同格式）。
fn new_run_id() -> Option<String> {
    new_prefixed_id("run")
}

fn new_prefixed_id(prefix: &str) -> Option<String> {
    let mut entropy = [0u8; 12];
    getrandom::fill(&mut entropy).ok()?;
    let hex: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("{prefix}_{hex}"))
}
