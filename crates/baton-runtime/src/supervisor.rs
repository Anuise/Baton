//! Supervisor：launch intent → 啟動 → 回執 → 就緒 → 停止；程序身分核對（spec 「程序與模組架構」）。
//!
//! 每次 `tick` 依序做七件事，而且每一件都是冪等的、只信任資料庫中持久化的狀態：
//! 1. **啟動**派工 Outbox 裡的 Run：先在交易中持久化 launch intent 與 command_id，才在交易外 spawn，
//!    再把回執寫回。Runtime 在中間任何一點當掉，下一次 tick 都用 command_id 找回原程序，**絕不重新啟動**；
//!    找不到程序時把 Run 標為 interrupted，交給使用者重新派工，而不是猜測「應該沒啟動」。
//! 2. **收事件**：接上 Adapter 的事件串流，heartbeat 續約 lease，限流訊號與用量記為該 Run 帳號的額度觀測，
//!    結束事件只讓 Run 結束，不把 Task 標為 completed。
//! 3. **取消**：對要求取消的 Run 停止整個程序樹；確認停止之後才完成取消、釋放寫入所有權。
//! 4. **核對 lease**：lease 過期只是「該去看看」：向 ProcessControl 查證，確認死亡才中斷 Run，存活就續約，不確定就維持原狀。
//! 5. **額度**：輪詢到期（每個帳號每 5 分鐘）的外部額度觀測器並重新推估（票 27）。
//! 6. **排程**：前面幾步可能釋放了帳號與全域名額，於是讓佇列中等待的 Task 依優先序逐一嘗試派出（票 19）；
//!    派出的 Run 在同一次 `tick` 就啟動。
//! 7. **驗證**（票 13）：對 `verifying` 且契約宣告了確定性檢查的 Task 執行檢查，以成果接受交易提交結果；
//!    交易會校驗 Run 世代、契約版本與成果 manifest，觀察之後狀態已經改變（例如使用者先手動接受）就被拒絕。
//!    檢查可能很久（命令最多 10 分鐘），所以背景迴圈把這一步放在另一個工作裡執行，不擋住前六步；
//!    測試呼叫的 `tick` 則等它做完。
//!
//! 正式執行時由背景迴圈呼叫 `tick`（有新的派工或取消時會被喚醒）；測試 harness 不啟動背景迴圈，
//! 由測試在想要的時刻呼叫 `tick`，搭配 fake Clock 就能完全決定性地重現任何時序。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use baton_api::{quota, LogSink, RunSettings};
use baton_domain::agent::{AGENT_TOKEN_ENV, RUNTIME_URL_ENV};
use baton_domain::auth::AgentScope;
use baton_domain::quota::RateLimitSignal;
use baton_domain::run::{LaunchPayload, ProcessObservation, ProcessRecord, Run, RunCommand, RunOutcome, RunStatus};
use baton_domain::verification::AcceptanceCommand;
use baton_domain::Timestamp;
use baton_ports::{
    AdapterEvent, AgentHandle, Liveness, Ports, ProcessIdentity, RunSpec, SpawnSpec, StopResult,
};
use baton_store::{ExecutingRun, PendingLaunch, Store};
use futures::stream::BoxStream;
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
    /// Run 設定快照（與 API 共用，同一個 Run 的快照請求依序處理）。
    run_settings: Arc<RunSettings>,
    /// 要求程序停止時給它的寬限期。
    stop_grace: Duration,
    /// 同一時間只有一個 tick 在進行（背景迴圈與測試呼叫不會交錯）。
    tick_lock: Mutex<()>,
    /// 同一時間只有一輪驗證在進行。
    verify_lock: Arc<Mutex<()>>,
    /// 成果儲存的根目錄（成果檢查從這裡讀取檔案）。
    artifacts_root: PathBuf,
    /// 已接上的事件串流；Runtime 重建後是空的，下一次 tick 會重新接上。
    streams: Mutex<HashMap<String, BoxStream<'static, AdapterEvent>>>,
    /// 這個 Runtime 實例已經向 ProcessControl 「找回」過（取得管理權）的 Run。
    adopted: Mutex<HashSet<String>>,
    /// 每個 Run 最近一次 Adapter 回報的錯誤；程序以失敗結束時作為失敗原因。
    last_errors: Mutex<HashMap<String, String>>,
}

impl Supervisor {
    pub fn new(
        store: Store,
        ports: Ports,
        log: LogSink,
        credentials: CredentialIssuer,
        runtime_url: String,
        run_settings: Arc<RunSettings>,
        stop_grace: Duration,
        artifacts_root: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            ports,
            log,
            credentials,
            runtime_url,
            run_settings,
            stop_grace,
            tick_lock: Mutex::new(()),
            verify_lock: Arc::new(Mutex::new(())),
            artifacts_root,
            streams: Mutex::new(HashMap::new()),
            adopted: Mutex::new(HashSet::new()),
            last_errors: Mutex::new(HashMap::new()),
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
                supervisor.supervise_once().await;
                // 驗證在另一個工作裡進行；上一輪還沒做完就不再開新的一輪
                if let Ok(guard) = supervisor.verify_lock.clone().try_lock_owned() {
                    let verifier = supervisor.clone();
                    tokio::spawn(async move {
                        verifier.verify_pending().await;
                        drop(guard);
                    });
                }
            }
        })
    }

    /// 處理目前所有待辦（含驗證）；每個步驟的失敗只記日誌，下一次 tick 會自然重試。
    pub async fn tick(&self) {
        self.supervise_once().await;
        let _serial = self.verify_lock.lock().await;
        self.verify_pending().await;
    }

    /// 驗證以外的六個步驟。
    async fn supervise_once(&self) {
        let _serial = self.tick_lock.lock().await;
        self.launch_pending().await;
        self.pump_events().await;
        self.process_cancellations().await;
        self.check_leases().await;
        self.poll_quota().await;
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

    // ------------------------------------------------------------ 額度

    /// 輪詢到期的帳號；沒有到期的帳號時什麼都不做。
    async fn poll_quota(&self) {
        if let Err(error) = quota::poll_due(&self.store, &*self.ports.quota_observer, self.ports.clock.now()).await {
            self.note(&format!("額度輪詢失敗：{error}"));
        }
    }

    /// 把 Run 回報的限流訊號或用量記為該 Run 帳號的額度觀測；沒有帳號的舊 Run（票 19 之前）略過。
    async fn record_quota_event(&self, run: &Run, event: AdapterEvent) {
        if run.profile_id.is_empty() {
            return;
        }
        let now = self.ports.clock.now();
        let result = match event {
            AdapterEvent::RateLimited { window, status, used, reset_at_millis, raw } => {
                let signal = RateLimitSignal { window, status, used, reset_at: reset_at_millis.map(Timestamp::from_millis), raw };
                let observation = baton_domain::quota::observe_signal(&run.profile_id, &run.id, &signal, now);
                self.store.record_quota_signal(observation).await
            }
            AdapterEvent::UsageReported { tokens, raw } => {
                self.store.record_quota_usage(run.profile_id.clone(), run.id.clone(), tokens, raw, now).await
            }
            _ => return,
        };
        if let Err(error) = result {
            self.note(&format!("Run {} 的額度觀測沒有記下：{error}", run.id));
        }
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
        let quota = match quota::current(&self.store, &*self.ports.quota_observer, self.ports.clock.now()).await {
            Ok(status) => quota::known_used(&status),
            Err(error) => {
                self.note(&format!("無法取得額度觀測：{error}"));
                return false;
            }
        };
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

    /// 組出 Run 的 `RunSpec`：全部來自持久化的狀態（派工 Outbox、Run 的帳號、設定快照的綁定），
    /// 所以啟動時與 Runtime 重建後組出的結果相同，Adapter 才能據此找回同一個程序。
    async fn run_spec(&self, run: &Run, payload: &LaunchPayload, command_id: &str) -> Result<RunSpec, String> {
        let profiles = self.store.list_profiles().await.map_err(|e| format!("無法讀取帳號 Profile：{e}"))?;
        let config_dir = profiles
            .into_iter()
            .find(|p| p.id == run.profile_id)
            .map(|p| p.config_dir)
            .ok_or_else(|| format!("找不到 Run 的帳號 Profile {}", run.profile_id))?;
        let pin = self.store.run_settings_pin(&run.id).await.map_err(|e| format!("無法讀取設定快照的綁定：{e}"))?;
        Ok(RunSpec {
            run_id: run.id.clone(),
            task_id: payload.task_id.clone(),
            command_id: command_id.to_string(),
            prompt: payload.prompt.clone(),
            working_dir: Some(payload.working_dir.clone()),
            config_dir: Some(config_dir),
            settings_dir: pin.map(|p| p.directory),
            model: payload.model.clone(),
        })
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
        // 2. 准入的共同設定：第一次啟動時以最新版本建立 Run 專屬唯讀快照，已綁定的依 manifest 核對（D-09）；
        //    核對不過就不啟動，不以不確定的設定執行
        if let Err(reason) = self.run_settings.for_launch(&item.run.id).await {
            let command = RunCommand::LaunchFailed { run_id: item.run.id.clone(), reason };
            return self.apply(&item.run, command).await;
        }
        let run_spec = match self.run_spec(&item.run, &item.payload, &command_id).await {
            Ok(spec) => spec,
            Err(reason) => {
                return self.apply(&item.run, RunCommand::LaunchFailed { run_id: item.run.id.clone(), reason }).await
            }
        };
        if let Err(error) = self.ports.agent_adapter.prepare(&run_spec).await {
            let reason = format!("無法準備 Run 的啟動檔案：{error}");
            return self.apply(&item.run, RunCommand::LaunchFailed { run_id: item.run.id.clone(), reason }).await;
        }
        // 3. Run 准入：簽發綁定這個 Run 與世代的 Agent 憑證，只經環境變數交給 MCP shim。
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
        // 4. 交易外啟動程序
        let mut spec = self.ports.agent_adapter.spawn_spec(&run_spec);
        spec.env.push((AGENT_TOKEN_ENV.to_string(), token));
        spec.env.push((RUNTIME_URL_ENV.to_string(), self.runtime_url.clone()));
        match self.ports.process_control.spawn(spec).await {
            // 5. 回執寫回；寫回之前當掉的話，下一次 tick 以 command_id 找回
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
        let run_spec = match self.run_spec(&item.run, &item.payload, &command_id).await {
            Ok(spec) => spec,
            Err(error) => return self.note(&format!("Run {} 暫時無法核對程序，稍後重試：{error}", item.run.id)),
        };
        let spec = self.ports.agent_adapter.spawn_spec(&run_spec);
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
                    AdapterEvent::Started { native_session_id: Some(native_session_id) } => {
                        // 條件綁定：第一次綁定，之後相同不變，不同只留下衝突紀錄
                        let command = RunCommand::BindNativeSession {
                            run_id: run_id.clone(),
                            generation: item.view.run.generation,
                            native_session_id,
                        };
                        self.apply(&item.view.run, command).await;
                    }
                    AdapterEvent::Error { message } => {
                        self.note(&format!("Run {run_id} 回報錯誤：{message}"));
                        self.last_errors.lock().await.insert(run_id.clone(), message);
                    }
                    AdapterEvent::Exited { success } => {
                        // 程序結束只產生 Run 結束事件；Task 是否完成由後續的驗證決定
                        let last_error = self.last_errors.lock().await.remove(&run_id);
                        let outcome = if success {
                            RunOutcome::Succeeded
                        } else {
                            RunOutcome::Failed { reason: last_error.unwrap_or_else(|| "程序以失敗結束".into()) }
                        };
                        let command =
                            RunCommand::Finish { run_id: run_id.clone(), generation: item.view.run.generation, outcome };
                        self.apply(&item.view.run, command).await;
                        finished = true;
                        break;
                    }
                    event @ (AdapterEvent::RateLimited { .. } | AdapterEvent::UsageReported { .. }) => {
                        self.record_quota_event(&item.view.run, event).await;
                    }
                    // 訊息由後續的票處理（對話紀錄）
                    AdapterEvent::Started { native_session_id: None } | AdapterEvent::Message { .. } => {}
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
        let spec = match self.run_spec(run, &item.payload, &command_id).await {
            Ok(spec) => spec,
            Err(error) => {
                self.note(&format!("Run {} 無法接上事件串流：{error}", run.id));
                return None;
            }
        };
        match self.ports.agent_adapter.launch(spec).await {
            Ok(handle) => Some(self.ports.agent_adapter.events(&AgentHandle(handle.0))),
            Err(error) => {
                self.note(&format!("Run {} 無法接上事件串流：{error}", run.id));
                None
            }
        }
    }

    // ------------------------------------------------------------ 驗證

    /// 對等待檢查的 Task 執行確定性檢查並提交結果；呼叫端負責持有 `verify_lock`。
    /// Runtime 在檢查途中當掉時沒有任何紀錄，下一次會重新執行檢查（不猜測結果）。
    async fn verify_pending(&self) {
        let pending = match self.store.pending_verifications().await {
            Ok(pending) => pending,
            Err(error) => return self.note(&format!("無法讀取等待驗證的 Task：{error}")),
        };
        for item in pending {
            let task_id = item.pending.task_id.clone();
            let results = crate::verifier::run_checks(&self.ports, &self.artifacts_root, &item).await;
            let Some(record_id) = new_prefixed_id("ver") else {
                self.note("無法取得系統亂數，驗證結果稍後重試");
                continue;
            };
            let command = AcceptanceCommand::RecordChecks { task_id: task_id.clone(), basis: item.pending.basis, results };
            match self.store.apply_acceptance(item.workspace_id, command, record_id, self.ports.clock.now()).await {
                Ok(change) => self.note(&format!(
                    "Task {task_id} 的確定性檢查結果：{}，Task 進入 {}",
                    change.record.outcome.as_str(),
                    change.task.status.as_str()
                )),
                // 觀察之後狀態已經改變（例如使用者先手動接受）：這份結果已過期，不套用
                Err(error) => self.note(&format!("Task {task_id} 的檢查結果沒有套用：{error}")),
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
        let Ok(run_spec) = self.run_spec(run, &item.payload, &command_id).await else { return };
        let spec: SpawnSpec = self.ports.agent_adapter.spawn_spec(&run_spec);
        if self.ports.process_control.find(&spec).await.is_ok() {
            self.adopted.lock().await.insert(run.id.clone());
        }
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
