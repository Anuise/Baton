//! Supervisor：launch intent → 啟動 → 回執 → 就緒 → 停止；程序身分核對（spec 「程序與模組架構」）。
//!
//! 每次 `tick` 依序做七件事，而且每一件都是冪等的、只信任資料庫中持久化的狀態：
//! 1. **啟動**派工 Outbox 裡的 Run：先在交易中持久化 launch intent 與 command_id，才在交易外 spawn，
//!    再把回執寫回。Runtime 在中間任何一點當掉，下一次 tick 都用 command_id 找回原程序，**絕不重新啟動**；
//!    找不到程序時把 Run 標為 interrupted，交給使用者重新派工，而不是猜測「應該沒啟動」。
//! 2. **收事件**：接上 Adapter 的事件串流，heartbeat 續約 lease，限流訊號與用量記為該 Run 帳號的額度觀測，
//!    對話紀錄依事件位置去重、遮蔽秘密後匯入成果儲存（票 15），結束事件只讓 Run 結束，不把 Task 標為 completed。
//! 3. **取消**：對要求取消的 Run 停止整個程序樹；確認停止之後才完成取消、釋放寫入所有權。
//! 4. **核對 lease**：lease 過期只是「該去看看」：向 ProcessControl 查證，確認死亡才中斷 Run，存活就續約，不確定就維持原狀。
//! 5. **額度**：輪詢到期（每個帳號每 5 分鐘）的外部額度觀測器並重新推估（票 27）。
//!    5½. **交接**（票 28）：依最新的額度判斷推進帳號門檻；draining 帳號上執行中的 Run 建立交接（已選定承接帳號），
//!    在安全邊界（舊 Worker 保存了 checkpoint 或等滿上限）先持久化 `stopping` 再要求舊程序停止，**確認停止**（或程序自行結束）
//!    之後以單一交易結束舊 Run、建立 successor Run 並把世代轉給它；停不下來就留在 `stopping`，successor 不會被建立。
//!    每個 phase 都先持久化，Runtime 在任何一點重建後都從持久化的 phase 繼續。
//!    突然限流（票 29，CLI 回報帳號已達上限）不受冷卻限制、不等 checkpoint；程序已自行結束時在收事件的這一步就建立交接並轉移。
//!    轉移時所有合適帳號都受限，Task 進入 `waiting_resource`，到了重新評估的時間（重置後 1 分鐘，或 5～60 分鐘的退避）
//!    才再找承接帳號；停止意圖或 Workspace 等待「繼續工作」時不派工。
//! 6. **排程**：前面幾步可能釋放了帳號與全域名額，於是讓佇列中等待的 Task 依優先序逐一嘗試派出（票 19）；
//!    派出的 Run 在同一次 `tick` 就啟動。派出之前先為排隊的 Task 準備工作目錄（worktree 或輸出目錄，票 20），
//!    目錄不可用時不派工，等待原因就是登記的失敗原因。
//! 7. **驗證**（票 13）：對 `verifying` 且契約宣告了確定性檢查的 Task 執行檢查，以成果接受交易提交結果；
//!    交易會校驗 Run 世代、契約版本與成果 manifest，觀察之後狀態已經改變（例如使用者先手動接受）就被拒絕。
//!    檢查可能很久（命令最多 10 分鐘），所以背景迴圈把這一步放在另一個工作裡執行，不擋住前六步；
//!    測試呼叫的 `tick` 則等它做完。
//! 8. **整合**（票 20）：在驗證之後、同一個工作中依序處理單一整合佇列，一次一個項目：先持久化「開始合併」，
//!    在整合目錄合併、執行整合驗證，通過才把目標分支前移並讓 Task 完成。崩潰後停在合併或驗證的項目從合併重做，
//!    停在前移的項目以同一個合併結果重試前移（前移是冪等的）。
//! 9. **清理**（票 21）：在整合之後、同一個工作中，把已完成並整合、Run 全部結束的開發 Task 的 worktree 登記為待清理，
//!    再處理到期的清理：先持久化「清理中」，重新核對安全條件（仍是平台登記的 worktree、沒有未保存或歸屬不明的檔案），
//!    符合才移除；失敗時記下原因、依退避有限重試，Task 仍是已完成。崩潰後停在清理中的登記重新核對後接續。
//!
//! 10. **Controller CLI**（desktop-controller-cli 票 03）：在收事件之後核對互動式 Controller Run 的開窗、宿主存活與結束（`controller_cli` 子模組）。
//!     Runtime 不再自動喚醒 Controller：舊的事件合併、`claude -p` 的 Controller 輪次與 Controller 自動交接已在票 08 移除。
//!
//! 正式執行時由背景迴圈呼叫 `tick`（有新的派工或取消時會被喚醒）；測試 harness 不啟動背景迴圈，
//! 由測試在想要的時刻呼叫 `tick`，搭配 fake Clock 就能完全決定性地重現任何時序。
//!
//! 另有兩件生命週期的工作（票 25）：
//! - **完全退出**（`full_exit`）：停止意圖已持久化後，先鏡像到生命週期檔，再要求每個 Run 在寬限期內停止、
//!   逾時終止程序樹並確認（spec D-11，總上限 40 秒）；無法確認停止的記為停止異常。期間握住 tick 的鎖，不會有任何啟動。
//! - **重新開啟**：Runtime 啟動時若停止意圖仍生效，第一次 `tick` 先核對（找回啟動中的程序、查證上次沒能確認停止的程序），
//!   再解除停止意圖，依各 Workspace 的「完全退出後自動續跑」設定續跑或等「繼續工作」。
//!
//! 以及崩潰恢復（票 31）：Runtime 啟動時若判定上次意外終止，就有一個未結束的恢復事件。`tick` 在任何啟動之前先處理它：
//! 退避期滿就開始一次嘗試——先持久化嘗試序號，再盤點核對（找回啟動中的程序、收下已到達的回報），
//! 受控停止身分已確認的舊程序，確認停止後 Run 結束、Task 重新排隊，由排程以新 Run 恢復（重新經過授權、設定與相依檢查）。
//! 任何一個舊程序無法確認停止，這次嘗試就失敗（不建立新 Run，不會有第二個 writer）。恢復尚未完成或額度用盡時不啟動也不派工；
//! 處置完成後穩定觀察 10 分鐘，期滿才持久化成功。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use baton_api::{quota, LogSink, RunSettings};
use baton_domain::agent::{AGENT_TOKEN_ENV, RUNTIME_URL_ENV};
use baton_domain::auth::AgentScope;
use baton_domain::cleanup::{self, CleanupCommand, CleanupStatus, Inspection};
use baton_domain::integration::{self, AdvanceResult, IntegrationCommand, IntegrationStatus, MergeResult};
use baton_domain::lifecycle::{
    ExitReport, ExitTiming, LifecyclePhase, RunStopOutcome, RunStopRecord, StopAnomaly,
    RUN_GONE_AFTER_EXIT_REASON,
};
use baton_domain::handoff::HandoffPhase;
use baton_domain::quota::{LimitStatus, RateLimitSignal};
use baton_domain::recovery::{self, AffectedRun, RecoveryCommand, RecoveryEvent, RecoveryStatus, RunDisposition};
use baton_domain::run::{
    LaunchPayload, ProcessObservation, ProcessRecord, Run, RunCommand, RunOutcome, RunStatus, RunView,
};
use baton_domain::session::{
    self, ResumeCapability, ResumeCheck, SegmentAvailability, SegmentPlan, TranscriptEntry, TranscriptSegment,
};
use baton_domain::verification::AcceptanceCommand;
use baton_domain::Timestamp;
use baton_ports::{
    AdapterEvent, AdvanceOutcome, AgentHandle, Capability, Liveness, MergeOutcome, Ports, ProcessIdentity, RunSpec,
    SessionRef, SpawnSpec, StopResult, WorktreeState,
};
use baton_store::{CleanupWork, ExecutingRun, HandoffWork, IntegrationWork, PendingLaunch, Store};
use futures::stream::{BoxStream, FuturesUnordered};
use futures::{FutureExt, StreamExt};
use tokio::sync::{watch, Mutex, Notify};
use tokio::task::JoinHandle;

use crate::CredentialIssuer;

mod controller_cli;

/// 對話紀錄在成果儲存中的子目錄（票 15）；檔名是內容的 SHA-256。
pub const TRANSCRIPTS_DIR: &str = "transcripts";

/// 受控停止的預設寬限期（spec D-11）；逾時由 ProcessControl 以 Job Object 強制終止整個程序樹。
pub const DEFAULT_STOP_GRACE: Duration = Duration::from_secs(30);

/// Worker 回報結束後，等它的程序樹自行結束的上限；之後終止 Job 內殘留的程序（例如放到背景的子程序）。
const EXIT_RELEASE_GRACE: Duration = Duration::from_secs(5);

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
    /// Adapter 回報過額度用盡的 Run：之後以失敗結束是突然限流，不是實作失敗（票 36）。
    quota_limited: Mutex<HashSet<String>>,
    /// 完全退出的時間參數與生命週期檔所在的資料目錄。
    exit_timing: ExitTiming,
    data_dir: PathBuf,
    /// 啟動時停止意圖仍生效：記下當時的生命週期版本，第一次 tick 核對完後重新開啟（版本變了就不重新開啟）。
    reopen_from: Mutex<Option<i64>>,
    /// 完全退出的停止流程正在等待或進行：握著 tick 鎖、可能久等的步驟（取消時的停止程序）看到就讓出，交給停止流程。
    exiting: watch::Sender<bool>,
    /// 最近一次寫給監護的恢復事件（識別與版本，票 32）；事件有變動才重寫鏡像。
    mirrored: Mutex<Option<(String, i64)>>,
    /// 這個 Runtime 實例自己開窗（簽發憑證）或重啟後已核對過的 Controller CLI（desktop-controller-cli 票 07）；
    /// 不在其中、又已寫過 launch intent 的，是上一個實例開出的，要先查驗宿主。
    controller_clis_checked: Mutex<HashSet<String>>,
}

/// Supervisor 的生命週期參數（票 25）。
pub struct LifecycleSettings {
    pub exit_timing: ExitTiming,
    pub data_dir: PathBuf,
    /// Runtime 啟動時看到的停止意圖版本；沒有停止意圖為 `None`。
    pub reopen_from: Option<i64>,
}

impl Supervisor {
    // 參數都是 Runtime 組好、各自獨立的相依，只有 `baton-runtime` 啟動時一個呼叫端；為此另包參數結構只是搬移欄位
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Store,
        ports: Ports,
        log: LogSink,
        credentials: CredentialIssuer,
        runtime_url: String,
        run_settings: Arc<RunSettings>,
        stop_grace: Duration,
        artifacts_root: PathBuf,
        lifecycle: LifecycleSettings,
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
            quota_limited: Mutex::new(HashSet::new()),
            exit_timing: lifecycle.exit_timing,
            data_dir: lifecycle.data_dir,
            reopen_from: Mutex::new(lifecycle.reopen_from),
            exiting: watch::Sender::new(false),
            mirrored: Mutex::new(None),
            controller_clis_checked: Mutex::new(HashSet::new()),
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
                // 驗證與整合在另一個工作裡進行；上一輪還沒做完就不再開新的一輪
                if let Ok(guard) = supervisor.verify_lock.clone().try_lock_owned() {
                    let verifier = supervisor.clone();
                    tokio::spawn(async move {
                        verifier.verify_pending().await;
                        verifier.revalidate_gates().await;
                        verifier.run_requested_verifications().await;
                        verifier.integrate_pending().await;
                        verifier.clean_pending().await;
                        drop(guard);
                    });
                }
            }
        })
    }

    /// 處理目前所有待辦（含驗證、整合與清理）；每個步驟的失敗只記日誌，下一次 tick 會自然重試。
    pub async fn tick(&self) {
        self.supervise_once().await;
        let _serial = self.verify_lock.lock().await;
        self.verify_pending().await;
        self.revalidate_gates().await;
        self.run_requested_verifications().await;
        self.integrate_pending().await;
        self.clean_pending().await;
    }

    /// 驗證以外的七個步驟。
    async fn supervise_once(&self) {
        let _serial = self.tick_lock.lock().await;
        self.check_storage().await;
        self.reopen_if_pending().await;
        self.recover_after_crash().await;
        // 崩潰恢復尚未完成或額度用盡：不啟動、不派工（Store 的准入也會擋，這裡先略過免得每次都被拒絕）
        let recovering = self.recovery_halts_dispatch().await;
        if !recovering {
            self.launch_pending().await;
        }
        let exited_into_handoff = self.pump_events().await;
        self.drive_controller_clis().await;
        self.process_cancellations().await;
        self.check_leases().await;
        self.poll_quota().await;
        // 交接也是派工：恢復擋住派工時不推進交接，舊程序交給恢復嘗試停止
        let handed_off = !recovering && self.hand_off().await;
        let scheduled = !recovering && self.schedule_queue().await;
        // 交接或排程派出的 Run 在同一次處理就啟動
        if !recovering && (exited_into_handoff || handed_off || scheduled) {
            self.launch_pending().await;
        }
    }

    /// 記下這一次啟動正常結束（Runtime 有秩序地停止服務時呼叫）；失敗只記日誌，下次啟動會保守地視為意外終止。
    pub async fn mark_clean_shutdown(&self) {
        if let Err(error) = self.store.mark_clean_shutdown(self.ports.clock.now()).await {
            self.note(&format!("無法記下正常結束：{error}"));
        }
    }

    /// Supervisor 使用的 Store（測試 harness 以它注入寫入故障）。
    pub fn store(&self) -> &Store {
        &self.store
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
        // 維護進行中時一併寫入維護抑制（票 34），不能被停止意圖的鏡像覆蓋掉
        let problem = match self.store.lifecycle_mirror(self.ports.clock.now()).await {
            Err(error) => Some(format!("無法讀取生命週期狀態：{error}")),
            Ok(file) => {
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

    /// 完全退出的停止流程（停止意圖已由 API 持久化在資料庫與生命週期檔）：再鏡像一次生命週期檔，停下全部尚未結束的 Run。
    /// 要求每個 Run 在寬限期內保存並停止，逾時由 ProcessControl 終止整個程序樹並確認；到總上限仍沒有結果的記為停止異常。
    /// 總上限從要求退出的那一刻（停止報告的 `deadline_at`）算起，等 tick 鎖也算在內；拿到鎖之後握到結束，
    /// 所以這段期間不會啟動任何程序。回傳最終的停止報告。
    pub async fn full_exit(&self) -> Option<ExitReport> {
        self.exiting.send_replace(true);
        let report = self.run_full_exit().await;
        self.exiting.send_replace(false);
        report
    }

    async fn run_full_exit(&self) -> Option<ExitReport> {
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
        // 期限以 Clock 持久化在停止報告中；換算成剩下的真實時間（Clock 已過期限則不再等待）
        let remaining = (report.deadline_at.as_millis() - self.ports.clock.now().as_millis()).max(0);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(remaining as u64);
        if let Some(problem) = self.mirror_lifecycle_file().await {
            report.notes.push(problem);
        }

        let serial = tokio::time::timeout_at(deadline, self.tick_lock.lock()).await;
        // 這個實例自己要退出了，不再做啟動時的重新開啟
        *self.reopen_from.lock().await = None;

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
        let mut stopping = FuturesUnordered::new();
        // 到期限仍拿不到 tick 鎖（另一個處理卡住）：不在它進行中時停止程序，全部記為無法確認
        let Ok(_serial) = serial else {
            report.notes.push("到上限仍等不到 Supervisor 的其他處理結束，沒有要求任何 Run 停止".to_string());
            return self.finish_exit(report, total).await;
        };
        for item in targets {
            // 找回程序（ProcessControl 的 find）同樣受總上限約束：到期限仍找不到，這個與之後的 Run 都在收尾記為無法確認
            let Ok(found) = tokio::time::timeout_at(deadline, self.process_of(&item)).await else {
                report.notes.push("到上限仍無法以 command_id 找回程序，其餘的 Run 沒有要求停止".to_string());
                break;
            };
            match found {
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

        // Controller CLI 也列入停止進度（desktop-controller-cli 票 07），與 Worker 同時結束
        let clis = match self.store.open_controller_cli_runs().await {
            Ok(clis) => clis,
            Err(error) => {
                report.notes.push(format!("無法讀取開著的 Controller CLI：{error}"));
                Vec::new()
            }
        };
        if !clis.is_empty() {
            for run in &clis {
                report.record(RunStopRecord::pending_controller_cli(&run.id, &run.workspace_id));
            }
            self.save_progress(&report).await;
        }
        let clis = tokio::time::timeout_at(deadline, self.stop_controller_clis_for_exit(clis));

        // 到總上限仍沒有結果就不再等待（逾時的那些在 `finish_exit` 記為停止異常）
        let workers = async {
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
        };
        let ((), clis) = tokio::join!(workers, clis);
        // 逾時的 Controller CLI 維持等待中，在 `finish_exit` 記為停止異常
        for record in clis.unwrap_or_default() {
            report.record(record);
        }
        self.finish_exit(report, total).await
    }

    /// 停止流程的收尾：仍沒有結果的 Run 記為停止異常 `stop_unconfirmed`（保留寫入所有權），寫入最終的停止報告。
    async fn finish_exit(&self, mut report: ExitReport, total: Duration) -> Option<ExitReport> {
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
        let run_spec = self
            .run_spec(&item.run, &item.payload, &command_id)
            .await
            .map_err(|error| format!("無法組出啟動中程序的啟動規格：{error}"))?;
        let spec = self.ports.agent_adapter.spawn_spec(&run_spec);
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

    // ------------------------------------------------------------ 崩潰恢復（票 31）

    /// 恢復是否擋住啟動與派工；讀不到恢復事件時保守地視為擋住。
    async fn recovery_halts_dispatch(&self) -> bool {
        match self.store.recovery_event().await {
            Ok(event) => recovery::launch_halted(event.as_ref()),
            Err(error) => {
                self.note(&format!("無法讀取恢復事件，這次不啟動也不派工：{error}"));
                true
            }
        }
    }

    /// 處理未結束的恢復事件：觀察期滿就持久化成功；等待中且退避期滿就做一次嘗試；
    /// 監護已計次的嘗試（啟動時匯入、狀態是嘗試中）由這個 Runtime 接續完成（票 32）。
    async fn recover_after_crash(&self) {
        let event = match self.store.recovery_event().await {
            Ok(Some(event)) => event,
            Ok(None) => return,
            Err(error) => return self.note(&format!("無法讀取恢復事件：{error}")),
        };
        // 使用者「重新嘗試恢復」等 API 路徑的變動也要讓監護看到
        self.mirror_recovery(&event).await;
        let now = self.ports.clock.now();
        match event.status {
            RecoveryStatus::Observing if event.observe_until.is_some_and(|until| now >= until) => {
                match self.store.apply_recovery(RecoveryCommand::Observe, now).await {
                    Ok(done) => {
                        if done.status == RecoveryStatus::Succeeded {
                            self.note(&format!("恢復事件 {} 已穩定 10 分鐘，恢復成功", done.id));
                        }
                        self.mirror_recovery(&done).await;
                    }
                    Err(error) => self.note(&format!("恢復事件 {} 的成功沒有寫入，下次再試：{error}", event.id)),
                }
            }
            RecoveryStatus::Waiting if event.next_attempt_at.is_none_or(|at| now >= at) => self.attempt_recovery().await,
            // 啟動時沒有被記為失敗的進行中嘗試，就是監護交給這個 Runtime 的那一次
            RecoveryStatus::Attempting if self.may_attempt_recovery().await => {
                self.note(&format!("恢復事件 {} 接續監護已計次的第 {} 次嘗試", event.id, event.attempts_used));
                self.carry_out_attempt().await;
            }
            _ => {}
        }
    }

    /// 控制資料庫無法寫入或停止意圖生效（完全退出優先）時不做恢復嘗試。
    async fn may_attempt_recovery(&self) -> bool {
        if self.store.storage_health().dispatch_halted {
            return false;
        }
        match self.store.lifecycle_state().await {
            Ok(state) => !state.stop_intent_active(),
            Err(error) => {
                self.note(&format!("無法讀取停止意圖，恢復嘗試稍後再開始：{error}"));
                false
            }
        }
    }

    /// 把恢復事件鏡像給監護（票 32）；這個版本已經寫成功過就略過，寫入失敗下一次再寫。
    async fn mirror_recovery(&self, event: &RecoveryEvent) {
        let key = (event.id.clone(), event.version);
        let mut mirrored = self.mirrored.lock().await;
        if mirrored.as_ref() == Some(&key) {
            return;
        }
        if !crate::write_recovery_mirror(&self.store, &self.data_dir, &self.ports, &self.log).await {
            return;
        }
        *mirrored = Some(key);
    }

    /// 一次完整的恢復嘗試。嘗試序號持久化之前不碰任何程序；中途當掉時，下次啟動會把這次嘗試記為失敗。
    async fn attempt_recovery(&self) {
        if !self.may_attempt_recovery().await {
            return;
        }
        let Some(attempt_id) = new_prefixed_id("rcv") else {
            return self.note("無法取得系統亂數，恢復嘗試稍後再開始");
        };
        // 1. 先持久化事件與嘗試序號；無法保存就不開始
        let begin = RecoveryCommand::Begin { attempt_id };
        let event = match self.store.apply_recovery(begin, self.ports.clock.now()).await {
            Ok(event) => event,
            Err(error) => return self.note(&format!("恢復嘗試沒有開始（無法保存嘗試序號）：{error}")),
        };
        self.note(&format!(
            "恢復事件 {} 第 {} 輪第 {}/{} 次嘗試開始",
            event.id,
            event.round,
            event.attempts_used,
            recovery::MAX_ATTEMPTS
        ));
        self.mirror_recovery(&event).await;
        self.carry_out_attempt().await;
    }

    /// 嘗試序號已持久化之後的處置：盤點核對、受控停止舊程序、記下結果。
    async fn carry_out_attempt(&self) {
        // 2. 盤點並核對：以 command_id 找回啟動中（回執未寫回）的程序，收下已到達的回報（保存證據；已自行結束的 Run 正常結束）
        self.reconcile_starting().await;
        self.pump_events().await;
        // 3. 受控停止身分已確認的舊程序；確認停止才讓 Task 重新排隊
        let (affected, failure) = self.stop_old_writers().await;
        // 4. 記下結果：全部確認停止就進入穩定觀察，否則這次嘗試失敗（退避或用盡）
        let command = match &failure {
            None => RecoveryCommand::Complete { affected },
            Some(reason) => RecoveryCommand::Fail { reason: reason.clone(), affected },
        };
        match self.store.apply_recovery(command, self.ports.clock.now()).await {
            Ok(event) => {
                let failure = failure.unwrap_or_default();
                match event.status {
                    RecoveryStatus::Observing => {
                        self.note(&format!("恢復事件 {} 的處置完成，開始穩定觀察 10 分鐘", event.id))
                    }
                    RecoveryStatus::Exhausted => self.note(&format!(
                        "恢復事件 {} 三次嘗試都沒有成功，停止自動恢復與派工，等使用者從恢復介面重新嘗試：{failure}",
                        event.id
                    )),
                    _ => self.note(&format!("恢復事件 {} 的這次嘗試失敗，退避後再試：{failure}", event.id)),
                }
                self.mirror_recovery(&event).await;
            }
            Err(error) => self.note(&format!("恢復嘗試的結果沒有寫入：{error}")),
        }
    }

    /// 只核對啟動中的 Run（不啟動排隊中的 Run）：找到原程序就補寫回執，確定沒有就中斷（不自動重新啟動）。
    async fn reconcile_starting(&self) {
        let pending = match self.store.pending_launches().await {
            Ok(pending) => pending,
            Err(error) => return self.note(&format!("無法讀取派工 Outbox：{error}")),
        };
        for item in pending.into_iter().filter(|item| item.run.status == RunStatus::Starting) {
            self.recover_launch(item).await;
        }
    }

    /// 受控停止所有尚未結束、已啟動的舊程序（同時進行，各自有寬限期）。回傳每個 Run 的處置，以及有無法確認停止時的失敗原因。
    async fn stop_old_writers(&self) -> (Vec<AffectedRun>, Option<String>) {
        let executing = match self.store.executing_runs().await {
            Ok(executing) => executing,
            Err(error) => return (Vec::new(), Some(format!("無法讀取執行中的 Run：{error}"))),
        };
        let mut affected = Vec::new();
        let mut stopping = FuturesUnordered::new();
        for item in executing {
            let run = item.view.run.clone();
            let Some(process) = run.process.as_ref() else {
                // 啟動中、而且這次無法以 command_id 查證：不能證明沒有程序，保留寫入所有權
                let message = "啟動結果不明，無法以 command_id 查證程序；保留寫入所有權".to_string();
                affected.push(affected_of(&run, RunDisposition::Unconfirmed, Some(message)));
                continue;
            };
            self.ensure_managed(&item).await;
            let identity = identity_of(process);
            let process_control = self.ports.process_control.clone();
            let grace = self.stop_grace;
            stopping.push(async move {
                let result = process_control.stop(&identity, grace).await;
                (run, result)
            });
        }
        while let Some((run, result)) = stopping.next().await {
            let unconfirmed = |message: String| affected_of(&run, RunDisposition::Unconfirmed, Some(message));
            let record = match result {
                Ok(StopResult::StoppedGracefully | StopResult::ForceKilled) => {
                    self.streams.lock().await.remove(&run.id);
                    match self.store.stop_run_for_recovery(&run.workspace_id, &run.id, self.ports.clock.now()).await {
                        Ok(change) if change.run.as_ref().is_some_and(|r| r.status == RunStatus::Cancelled) => {
                            affected_of(&run, RunDisposition::Cancelled, None)
                        }
                        Ok(_) => affected_of(&run, RunDisposition::Requeued, None),
                        Err(error) => unconfirmed(format!("程序已停止，但無法寫入 Run 的結束：{error}")),
                    }
                }
                Ok(StopResult::StillAlive) => unconfirmed("終止程序樹之後仍有程序存活；保留寫入所有權".to_string()),
                Err(error) => unconfirmed(format!("停止程序失敗：{error}；保留寫入所有權")),
            };
            affected.push(record);
        }
        let unconfirmed = affected.iter().filter(|a| a.disposition == RunDisposition::Unconfirmed).count();
        let failure = (unconfirmed > 0).then(|| format!("{unconfirmed} 個舊程序無法確認停止，沒有以新 Run 恢復它們的工作"));
        (affected, failure)
    }

    // ------------------------------------------------------------ 額度

    /// 輪詢到期的帳號；沒有到期的帳號時什麼都不做。
    async fn poll_quota(&self) {
        if let Err(error) = quota::poll_due(&self.store, &*self.ports.quota_observer, self.ports.clock.now()).await {
            self.note(&format!("額度輪詢失敗：{error}"));
        }
    }

    /// 把 Run（含 Controller Run）回報的限流訊號或用量記為該帳號的額度觀測；沒有帳號的舊 Run（票 19 之前）略過。
    async fn record_quota_event(&self, profile_id: &str, run_id: &str, event: AdapterEvent) {
        if profile_id.is_empty() {
            return;
        }
        let now = self.ports.clock.now();
        let result = match event {
            AdapterEvent::RateLimited { window, status, used, reset_at_millis, raw } => {
                let signal = RateLimitSignal { window, status, used, reset_at: reset_at_millis.map(Timestamp::from_millis), raw };
                let observation = baton_domain::quota::observe_signal(profile_id, run_id, &signal, now);
                self.store.record_quota_signal(observation).await
            }
            AdapterEvent::UsageReported { tokens, context_tokens, raw } => {
                self.store.record_quota_usage(profile_id.to_string(), run_id.to_string(), tokens, context_tokens, raw, now).await
            }
            _ => return,
        };
        if let Err(error) = result {
            self.note(&format!("Run {run_id} 的額度觀測沒有記下：{error}"));
        }
    }

    // ------------------------------------------------------------ 交接（票 28）

    /// 主動交接（issue §5.2）：推進額度門檻、為 draining 帳號上的 Run 建立交接、在安全邊界要求舊程序停止、
    /// 確認停止後轉移給 successor。回傳這次有沒有建立 successor Run。
    async fn hand_off(&self) -> bool {
        // 控制資料庫無法寫入時不交接：phase 寫不進去就不能做下一步的外部副作用（票 26）
        if self.store.storage_health().dispatch_halted {
            return false;
        }
        let now = self.ports.clock.now();
        let quota = match quota::current(&self.store, &*self.ports.quota_observer, now).await {
            Ok(status) => quota::known_used(&status),
            Err(error) => {
                self.note(&format!("無法取得額度觀測，這次不交接：{error}"));
                return false;
            }
        };
        match self.store.request_handoffs(quota.clone(), now, || new_prefixed_id("hof")).await {
            Ok(created) => {
                for handoff in created {
                    self.note(&format!(
                        "帳號 {} 進入 draining：Run {} 開始交接，承接帳號 {}",
                        handoff.from_profile_id, handoff.from_run_id, handoff.to_profile_id
                    ));
                }
            }
            Err(error) => self.note(&format!("無法建立交接：{error}")),
        }
        let works = match self.store.in_flight_handoffs().await {
            Ok(works) => works,
            Err(error) => {
                self.note(&format!("無法讀取進行中的交接：{error}"));
                return false;
            }
        };
        let mut transferred = self.resume_resource_waits(quota.clone(), now).await;
        for work in works {
            let stopping = match work.handoff.phase {
                HandoffPhase::Stopping => true,
                HandoffPhase::Requested => match self.store.begin_handoff_stop(&work.handoff.id, quota.clone(), now).await {
                    Ok(Some(next)) if next.phase == HandoffPhase::Stopping => true,
                    Ok(Some(next)) => {
                        let reason = next.abandoned_reason.unwrap_or_default();
                        self.note(&format!("Run {} 的交接已放棄：{reason}", work.from_run.id));
                        false
                    }
                    // 還沒到安全邊界或暫時沒有承接帳號：舊 Run 繼續執行
                    Ok(None) => false,
                    Err(error) => {
                        self.note(&format!("Run {} 的交接沒有前進：{error}", work.from_run.id));
                        false
                    }
                },
                _ => false,
            };
            if stopping {
                transferred |= self.stop_for_handoff(&work).await;
            }
        }
        transferred
    }

    /// 等待資源的交接（票 29）：到了重新評估的時間就找承接帳號，有就派出 successor。回傳這次有沒有建立 successor。
    async fn resume_resource_waits(&self, quota: HashMap<String, f64>, now: Timestamp) -> bool {
        let changed = match self.store.resume_resource_waits(quota, now, new_run_id).await {
            Ok(changed) => changed,
            Err(error) => {
                self.note(&format!("無法重新評估等待資源的交接：{error}"));
                return false;
            }
        };
        let mut transferred = false;
        for handoff in changed {
            match handoff.phase {
                HandoffPhase::Transferred => {
                    transferred = true;
                    self.note(&format!(
                        "交接 {}：資源恢復，由帳號 {} 的 Run {} 從檢查點接手",
                        handoff.id,
                        handoff.to_profile_id,
                        handoff.to_run_id.as_deref().unwrap_or("")
                    ));
                }
                HandoffPhase::WaitingResource => self.note(&format!(
                    "交接 {}：所有合適帳號仍受限，第 {} 次重新評估排在 {}",
                    handoff.id,
                    handoff.wait_attempts,
                    handoff.next_check_at.map_or(0, Timestamp::as_millis)
                )),
                _ => self.note(&format!(
                    "交接 {} 已放棄：{}",
                    handoff.id,
                    handoff.abandoned_reason.as_deref().unwrap_or_default()
                )),
            }
        }
        transferred
    }

    /// 突然限流的 Run 已自行結束、還沒有交接：先依最新的額度推進門檻，再為它建立交接（帳號不是突然限流時沒有）。
    async fn request_sudden_handoff(&self, run_id: &str) -> Option<baton_domain::handoff::Handoff> {
        let now = self.ports.clock.now();
        let quota = match quota::current(&self.store, &*self.ports.quota_observer, now).await {
            Ok(status) => quota::known_used(&status),
            Err(error) => {
                self.note(&format!("無法取得額度觀測，Run {run_id} 照一般結束處理：{error}"));
                return None;
            }
        };
        match self.store.request_sudden_handoff(run_id, quota, now, || new_prefixed_id("hof")).await {
            Ok(handoff) => handoff,
            Err(error) => {
                self.note(&format!("無法為突然限流的 Run {run_id} 建立交接，照一般結束處理：{error}"));
                None
            }
        }
    }

    /// 要求交接中的舊程序停止；**確認停止**才轉移（停不下來或停止失敗就留在 stopping，下一次再試，不會有第二個 writer）。
    async fn stop_for_handoff(&self, work: &HandoffWork) -> bool {
        let run = &work.from_run;
        // 舊 Run 已由別的路徑結束：轉移的交易會放棄這次交接
        let (Some(process), Some(payload)) =
            (run.process.as_ref().filter(|_| !run.status.is_terminal()), work.payload.as_ref())
        else {
            return self.transfer_handoff(&work.handoff.id, "already_ended").await;
        };
        let executing = ExecutingRun { view: RunView::new(run.clone(), None, &[]), payload: payload.clone() };
        self.ensure_managed(&executing).await;
        let identity = identity_of(process);
        // 先查證：Runtime 重建前已經停下的程序不必再要求停止
        let outcome = match self.ports.process_control.inspect(&identity).await {
            Ok(Liveness::Dead) => "dead",
            _ => {
                // 停止最長要等寬限期加確認上限；完全退出開始時就不再等，改由停止流程處理
                let mut exiting = self.exiting.subscribe();
                let result = tokio::select! {
                    result = self.ports.process_control.stop(&identity, self.stop_grace) => result,
                    _ = exiting.wait_for(|exiting| *exiting) => {
                        self.note(&format!("完全退出開始：Run {} 的交接停止改由停止流程處理", run.id));
                        return false;
                    }
                };
                match result {
                    Ok(StopResult::StoppedGracefully) => "stopped_gracefully",
                    Ok(StopResult::ForceKilled) => "force_killed",
                    Ok(StopResult::StillAlive) => {
                        self.note(&format!("Run {} 的程序沒有停止，交接停在 stopping，保留寫入所有權並稍後重試", run.id));
                        return false;
                    }
                    Err(error) => {
                        self.note(&format!("Run {} 停止程序失敗，交接停在 stopping：{error}", run.id));
                        return false;
                    }
                }
            }
        };
        self.streams.lock().await.remove(&run.id);
        self.transfer_handoff(&work.handoff.id, outcome).await
    }

    /// 舊程序已確認停止：以單一交易結束舊 Run、建立 successor Run。回傳有沒有建立 successor。
    async fn transfer_handoff(&self, handoff_id: &str, stop_outcome: &str) -> bool {
        let now = self.ports.clock.now();
        let quota = match quota::current(&self.store, &*self.ports.quota_observer, now).await {
            Ok(status) => quota::known_used(&status),
            Err(error) => {
                self.note(&format!("無法取得額度觀測，交接 {handoff_id} 稍後再轉移：{error}"));
                return false;
            }
        };
        match self.store.transfer_handoff(handoff_id, stop_outcome, quota, now, new_run_id).await {
            Ok(handoff) if handoff.phase == HandoffPhase::Transferred => {
                self.note(&format!(
                    "交接 {handoff_id}：Run {} 已停止（{stop_outcome}），由帳號 {} 的 Run {} 接手（第 {} 世代）",
                    handoff.from_run_id,
                    handoff.to_profile_id,
                    handoff.to_run_id.as_deref().unwrap_or(""),
                    handoff.to_generation.unwrap_or_default()
                ));
                true
            }
            Ok(handoff) if handoff.phase == HandoffPhase::WaitingResource => {
                self.note(&format!(
                    "交接 {handoff_id}：Run {} 已停止（{stop_outcome}），所有合適帳號都受限，Task 等待資源，預計 {} 重新評估",
                    handoff.from_run_id,
                    handoff.next_check_at.map_or(0, Timestamp::as_millis)
                ));
                false
            }
            Ok(handoff) => {
                let reason = handoff.abandoned_reason.unwrap_or_default();
                self.note(&format!("交接 {handoff_id} 沒有轉移（{}）：{reason}", handoff.phase.as_str()));
                false
            }
            Err(error) => {
                self.note(&format!("交接 {handoff_id} 無法轉移，稍後重試：{error}"));
                false
            }
        }
    }

    // ------------------------------------------------------------ 排程

    /// 讓佇列中等待的 Task 依序嘗試派出；回傳這次有沒有派出新的 Run。
    async fn schedule_queue(&self) -> bool {
        // 控制資料庫無法寫入時停止新派工（票 26）
        if self.store.storage_health().dispatch_halted {
            return false;
        }
        match self.store.has_dispatch_requests().await {
            Ok(true) => {}
            Ok(false) => return false,
            Err(error) => {
                self.note(&format!("無法讀取派工佇列：{error}"));
                return false;
            }
        }
        // 先為排隊的 Task 準備工作目錄；失敗只會被登記，派工判定會據此等待並顯示原因
        let now = self.ports.clock.now();
        if let Err(error) = baton_api::workdirs::prepare(&self.store, &*self.ports.workspace_fs, now, None).await {
            self.note(&format!("無法準備 Task 的工作目錄：{error}"));
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

    // ------------------------------------------------------------ 儲存故障（票 26）

    /// 控制資料庫先前寫入失敗（磁碟滿、被鎖住、無法寫入）：試寫一次，成功就解除新派工的停止。
    async fn check_storage(&self) {
        let Some(fault) = self.store.storage_health().fault else { return };
        match self.store.probe_storage(self.ports.clock.now()).await {
            Ok(()) => self.note(&format!("控制資料庫恢復可寫（先前：{}），恢復啟動程序與派工", fault.summary)),
            Err(error) => self.note(&format!("{}，暫停啟動程序與派工：{error}", fault.summary)),
        }
    }

    async fn launch_pending(&self) {
        // 無法寫入時不啟動任何程序：回執寫不回去就會產生無法追溯的程序（票 26）
        if self.store.storage_health().dispatch_halted {
            return;
        }
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
        let config_dir = self.config_dir_of(&run.profile_id).await?;
        let pin = self.store.run_settings_pin(&run.id).await.map_err(|e| format!("無法讀取設定快照的綁定：{e}"))?;
        // 快照中有哪些設定檔由綁定的版本決定（不讀檔案系統），重建後組出相同的啟動指令
        let settings_files = match &pin {
            Some(pin) => {
                let version = self
                    .store
                    .settings_version(pin.version)
                    .await
                    .map_err(|e| format!("無法讀取共同設定版本 {}：{e}", pin.version))?
                    .ok_or_else(|| format!("找不到 Run 綁定的共同設定版本 {}", pin.version))?;
                baton_domain::shared_settings::snapshot_files(&version)
                    .into_iter()
                    .map(|file| file.name)
                    .filter(|name| name != baton_domain::shared_settings::MANIFEST_FILE)
                    .collect()
            }
            None => Vec::new(),
        };
        Ok(RunSpec {
            run_id: run.id.clone(),
            task_id: payload.task_id.clone(),
            command_id: command_id.to_string(),
            prompt: payload.prompt.clone(),
            working_dir: Some(payload.working_dir.clone()),
            config_dir: Some(config_dir),
            settings_dir: pin.map(|p| p.directory),
            settings_files,
            model: payload.model.clone(),
            resume_native_session_id: run.resume_native_session_id.clone(),
            continue_session: false,
        })
    }

    /// 帳號 Profile 的 `CLAUDE_CONFIG_DIR`。
    async fn config_dir_of(&self, profile_id: &str) -> Result<String, String> {
        let profiles = self.store.list_profiles().await.map_err(|e| format!("無法讀取帳號 Profile：{e}"))?;
        profiles
            .into_iter()
            .find(|p| p.id == profile_id)
            .map(|p| p.config_dir)
            .ok_or_else(|| format!("找不到 Run 的帳號 Profile {profile_id}"))
    }

    /// 寫入 launch intent 之前，對可以沿用的 Session 查驗原生接續能力（票 15）。沒有候選時不需要查驗；
    /// Adapter 查驗失敗視為未知（不宣稱可續用）。讀不到資料時回傳錯誤，這次不啟動，免得因為暫時的錯誤換掉 Session。
    async fn resume_check(&self, run: &Run, payload: &LaunchPayload) -> Result<Option<ResumeCheck>, String> {
        let candidate = self
            .store
            .session_resume_candidate(&run.workspace_id, &run.task_id, &run.profile_id, &payload.working_dir)
            .await
            .map_err(|e| format!("無法讀取 Session：{e}"))?;
        let Some(candidate) = candidate else { return Ok(None) };
        let Some(native_session_id) = candidate.native_session_id.clone() else { return Ok(None) };
        let reference = SessionRef {
            native_session_id,
            config_dir: Some(self.config_dir_of(&run.profile_id).await?),
            working_dir: Some(payload.working_dir.clone()),
        };
        let capability = match self.ports.agent_adapter.resume_capability(&reference).await {
            Ok(Capability::Resumable) => ResumeCapability::Resumable,
            Ok(Capability::NotResumable) => ResumeCapability::NotResumable,
            Ok(Capability::Unknown) => ResumeCapability::Unknown,
            Err(error) => {
                self.note(&format!("Run {} 無法查驗 Session {} 能否接續，視為未知：{error}", run.id, candidate.id));
                ResumeCapability::Unknown
            }
        };
        Ok(Some(ResumeCheck { session_id: candidate.id, capability }))
    }

    async fn launch(&self, item: PendingLaunch) {
        let Some(command_id) = new_command_id() else {
            return self.note(&format!("Run {} 無法取得系統亂數，不啟動程序", item.run.id));
        };
        // 1. 先持久化 launch intent 與 command_id；這一步完成之前不會有任何程序被啟動。
        //    這也是准入（票 24）：同一個交易以這一刻生效的授權重新檢查，被拒時 Run 直接結束、Task 重新排隊。
        //    同一個交易中也決定沿用或建立 Session（票 15），所以原生識別取得前平台 Session 已經存在
        let resume_check = match self.resume_check(&item.run, &item.payload).await {
            Ok(check) => check,
            Err(error) => return self.note(&format!("Run {} 暫時無法決定 Session，稍後再啟動：{error}", item.run.id)),
        };
        let begin = RunCommand::BeginLaunch {
            run_id: item.run.id.clone(),
            command_id: command_id.clone(),
            working_dir: item.payload.working_dir.clone(),
            resume_check,
        };
        let launched = match self.store.apply_run_command(item.run.workspace_id.clone(), begin, self.ports.clock.now()).await {
            Err(error) => return self.note(&format!("Run {} 無法寫入 launch intent，不啟動程序：{error}", item.run.id)),
            Ok(change) => match change.run {
                Some(run) if run.status != RunStatus::Starting => {
                    let reason = run.end_reason.unwrap_or_default();
                    return self.note(&format!("Run {} 沒有通過啟動准入，不啟動程序：{reason}", item.run.id));
                }
                Some(run) => run,
                // 已經是同一個 launch intent（冪等）：不會發生，新的 command_id 每次都不同
                None => item.run.clone(),
            },
        };
        let settings_version = launched.settings_version;
        // 啟動規格以寫入 launch intent 後的 Run 組出（含要接續的原生 Session）
        let item = PendingLaunch { run: launched, payload: item.payload };
        // 2. 准入的共同設定：以准入時固定的版本建立 Run 專屬唯讀快照，已綁定的依 manifest 核對（D-09）；
        //    核對不過就不啟動，不以不確定的設定執行
        if let Err(reason) = self.run_settings.for_launch(&item.run.id, settings_version).await {
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
        let lookup = spec.clone();
        let receipt = match self.ports.process_control.spawn(spec).await {
            Ok(receipt) => receipt,
            // spawn 回報失敗不代表程序沒有啟動（例如回執在傳回途中遺失）：先以 command_id 查證（票 26）
            Err(error) => match self.ports.process_control.find(&lookup).await {
                Ok(Some(receipt)) => {
                    self.note(&format!("Run {} 的 spawn 回報失敗（{error}），但以 command_id 找到已啟動的程序", item.run.id));
                    receipt
                }
                // 確定沒有程序：啟動失敗
                Ok(None) => {
                    let command = RunCommand::LaunchFailed { run_id: item.run.id.clone(), reason: error.to_string() };
                    return self.apply(&item.run, command).await;
                }
                // 無法證明有沒有啟動：留在 starting 等核對，不重新啟動、不判定失敗
                Err(find_error) => {
                    return self.note(&format!(
                        "Run {} 的 spawn 回報失敗（{error}），也無法以 command_id 查證（{find_error}），留待下次核對",
                        item.run.id
                    ));
                }
            },
        };
        // 5. 回執寫回；寫回之前當掉的話，下一次 tick 以 command_id 找回
        let command = RunCommand::RecordReceipt { run_id: item.run.id.clone(), process: process_record(&receipt.identity) };
        self.apply(&item.run, command).await;
        self.adopted.lock().await.insert(item.run.id.clone());
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

    /// 回傳這次有沒有因為交接中的 Worker 結束而建立 successor Run。
    async fn pump_events(&self) -> bool {
        let mut transferred = false;
        let executing = match self.store.executing_runs().await {
            Ok(executing) => executing,
            Err(error) => {
                self.note(&format!("無法讀取執行中的 Run：{error}"));
                return false;
            }
        };
        // 已經結束的 Run（例如經 lease 核對或取消結束）不再需要事件串流與暫存的錯誤、限流訊號；
        // 留著的串流會讓 Adapter 的背景讀取一直進行
        let active: HashSet<String> = executing.iter().map(|e| e.view.run.id.clone()).collect();
        self.streams.lock().await.retain(|run_id, _| active.contains(run_id));
        self.last_errors.lock().await.retain(|run_id, _| active.contains(run_id));
        self.quota_limited.lock().await.retain(|run_id| active.contains(run_id));
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
            let mut transcript = Vec::new();
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
                        // 結束（或交接轉移）之前先確認整個程序樹都已結束：Worker 留在背景的子孫程序會繼續寫同一個
                        // worktree（D-10）。確認不了就把結束事件放回串流，下一次處理再試，這段期間仍是寫入所有者
                        if !self.release_process_tree(&item, EXIT_RELEASE_GRACE).await {
                            stream = Box::pin(futures::stream::iter([AdapterEvent::Exited { success }]).chain(stream));
                            break;
                        }
                        // 交接中的 Worker 結束（多半是收到 stop_requested 後保存 checkpoint 並結束）：程序已經不在，
                        // 視同舊程序確認停止，轉移給 successor；不論結束碼都不算失敗（票 28）
                        match self.store.in_flight_handoff_of_run(&run_id).await {
                            Ok(Some(handoff)) => {
                                self.last_errors.lock().await.remove(&run_id);
                                self.quota_limited.lock().await.remove(&run_id);
                                transferred |= self.transfer_handoff(&handoff.id, "exited").await;
                                finished = true;
                                break;
                            }
                            // 帳號已達上限後以失敗結束（突然限流，票 29）：不依賴最後一輪摘要，以最近的有效 checkpoint
                            // 交給其他帳號接手，或在所有帳號都受限時等待資源
                            Ok(None) if !success => {
                                if let Some(handoff) = self.request_sudden_handoff(&run_id).await {
                                    self.last_errors.lock().await.remove(&run_id);
                                    self.quota_limited.lock().await.remove(&run_id);
                                    transferred |= self.transfer_handoff(&handoff.id, "exited").await;
                                    finished = true;
                                    break;
                                }
                            }
                            Ok(None) => {}
                            // 讀不到交接紀錄：照一般的結束處理（Domain 仍以世代與狀態把關）
                            Err(error) => self.note(&format!("Run {run_id} 無法確認是否在交接中，照一般結束處理：{error}")),
                        }
                        // 程序結束只產生 Run 結束事件；Task 是否完成由後續的驗證決定
                        let last_error = self.last_errors.lock().await.remove(&run_id);
                        let limited = self.quota_limited.lock().await.remove(&run_id);
                        let reason = last_error.unwrap_or_else(|| "程序以失敗結束".into());
                        let outcome = match (success, limited) {
                            (true, _) => RunOutcome::Succeeded,
                            // 帳號額度用盡後才失敗：交給其他帳號接手，不計入任何預算
                            (false, true) => RunOutcome::QuotaLimited { reason },
                            (false, false) => RunOutcome::Failed { reason },
                        };
                        let command =
                            RunCommand::Finish { run_id: run_id.clone(), generation: item.view.run.generation, outcome };
                        self.apply(&item.view.run, command).await;
                        finished = true;
                        break;
                    }
                    event @ (AdapterEvent::RateLimited { .. } | AdapterEvent::UsageReported { .. }) => {
                        if matches!(event, AdapterEvent::RateLimited { status: LimitStatus::Exhausted, .. }) {
                            self.quota_limited.lock().await.insert(run_id.clone());
                        }
                        self.record_quota_event(&item.view.run.profile_id, &item.view.run.id, event).await;
                    }
                    // 訊息由後續的票處理（對話紀錄）
                    AdapterEvent::Transcript { position, line } => transcript.push(TranscriptEntry { position, line }),
                    AdapterEvent::Started { native_session_id: None } | AdapterEvent::Message { .. } => {}
                }
            }
            if !transcript.is_empty() {
                self.import_transcript(&item.view.run, &transcript).await;
            }
            if !finished {
                self.streams.lock().await.insert(run_id, stream);
            }
        }
        transferred
    }

    /// 把一批對話紀錄匯入受控儲存（票 15）：已保存過的位置略過（重新接上串流時的重播）、跳號記為缺口，
    /// 內容遮蔽秘密後以雜湊命名寫入成果儲存，再記錄段落。寫入失敗時停在那裡，之後到達的位置會把沒保存的部分記成缺口。
    async fn import_transcript(&self, run: &Run, entries: &[TranscriptEntry]) {
        let progress = match self.store.transcript_progress(&run.id).await {
            Ok(Some(progress)) => progress,
            Ok(None) => return self.note(&format!("Run {} 沒有 Session，對話紀錄沒有匯入", run.id)),
            Err(error) => return self.note(&format!("Run {} 無法讀取對話紀錄的進度：{error}", run.id)),
        };
        let now = self.ports.clock.now();
        let mut segments = Vec::new();
        for plan in session::plan_transcript_import(progress.imported_through, entries) {
            let segment = |first_position, last_position, availability, stored: Option<(String, String, u64)>, entries, redactions| {
                let (storage_path, sha256, byte_len) = match stored {
                    Some((path, sha, len)) => (Some(path), Some(sha), Some(len)),
                    None => (None, None, None),
                };
                TranscriptSegment {
                    agent_session_id: progress.agent_session_id.clone(),
                    run_id: run.id.clone(),
                    first_position,
                    last_position,
                    availability,
                    storage_path,
                    sha256,
                    byte_len,
                    entry_count: entries,
                    redactions,
                    recorded_at: now,
                }
            };
            match plan {
                SegmentPlan::Missing { first_position, last_position } => {
                    segments.push(segment(first_position, last_position, SegmentAvailability::Missing, None, 0, 0));
                }
                SegmentPlan::Stored { first_position, last_position, content, entry_count, redactions } => {
                    let sha256 = baton_domain::artifact::sha256_hex(content.as_bytes());
                    let storage_path = format!("{TRANSCRIPTS_DIR}/{sha256}.jsonl");
                    let target = self.artifacts_root.join(&storage_path);
                    if let Err(error) = self.ports.workspace_fs.write_artifact_atomic(&target, content.as_bytes()).await {
                        self.note(&format!("Run {} 的對話紀錄（位置 {first_position}–{last_position}）無法保存：{error}", run.id));
                        break;
                    }
                    let stored = Some((storage_path, sha256, content.len() as u64));
                    segments.push(segment(first_position, last_position, SegmentAvailability::Stored, stored, entry_count, redactions));
                }
            }
        }
        if let Err(error) = self.store.record_transcript_segments(segments, now).await {
            self.note(&format!("Run {} 的對話紀錄段落沒有記錄：{error}", run.id));
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

    /// 重新查驗已通過的 Gate（票 50）；呼叫端負責持有 `verify_lock`。先在交易外觀察受驗目錄目前的 build（觀察失敗的目錄
    /// 這次不比較），再由 Store 在一個交易中判定失效、退回 Stage、排入重驗的驗證要求並通知 Controller。
    /// 排在執行驗證要求之前：這一輪排入的重驗會在同一輪執行。
    async fn revalidate_gates(&self) {
        let dirs = match self.store.gate_build_dirs().await {
            Ok(dirs) => dirs,
            Err(error) => return self.note(&format!("無法讀取要重新查驗的 Gate：{error}")),
        };
        let mut builds = baton_store::Builds::new();
        for dir in dirs {
            match self.ports.verification_driver.revision(std::path::Path::new(&dir)).await {
                Ok(revision) => {
                    builds.insert(dir, revision);
                }
                Err(error) => self.note(&format!("無法讀取 {dir} 目前的 build，這次不比較：{error}")),
            }
        }
        match self.store.revalidate_gates(builds, self.ports.clock.now()).await {
            Ok(done) => {
                for workflow in done {
                    self.note(&format!(
                        "Workflow {} 的 Gate 失效、需要重驗：{}{}",
                        workflow.workflow_id,
                        workflow.stages.join("、"),
                        if workflow.verification_requests.is_empty() {
                            String::new()
                        } else {
                            format!("（已排入驗證要求 {}）", workflow.verification_requests.join("、"))
                        }
                    ));
                }
            }
            Err(error) => self.note(&format!("無法重新查驗 Gate：{error}")),
        }
    }

    /// 依 Verification Profile 執行等待中的驗證要求（票 45），保存 Evidence Bundle；呼叫端負責持有 `verify_lock`。
    /// 每個要求以一次或多次嘗試執行（票 49）：語意 judge 限流而另有帳號可用時，那次嘗試停止它的測試程序後交接，
    /// 立刻以另一個帳號開始新的嘗試。Runtime 在執行途中當掉時要求仍是 queued，那次嘗試仍是執行中；下一次開始新的嘗試時
    /// 把它記為中斷，承接者依它的日誌停止留下的測試程序、沿用已完成的旅程、不重送已開始的外部操作。
    async fn run_requested_verifications(&self) {
        let pending = match self.store.pending_verification_runs().await {
            Ok(pending) => pending,
            Err(error) => return self.note(&format!("無法讀取等待執行的驗證要求：{error}")),
        };
        for item in pending {
            self.run_verification_request(&item).await;
        }
    }

    /// 執行一個驗證要求直到產生 Bundle，或這次無法繼續（下一次 tick 再接手）。每次交接都排除限流的帳號，
    /// 所以交接次數不超過帳號數。
    async fn run_verification_request(&self, item: &baton_store::PendingVerificationRun) {
        let request_id = item.request.id.as_str();
        loop {
            let previous = match self.store.verification_attempts(request_id).await {
                Ok(previous) => previous,
                Err(error) => return self.note(&format!("無法讀取驗證要求 {request_id} 的嘗試：{error}")),
            };
            let excluded = baton_domain::verification_attempt::rate_limited_accounts(&previous);
            let judge_account = match crate::verification_adapter::judge_accounts(&self.store, &excluded).await {
                Ok(accounts) => accounts.into_iter().next(),
                Err(error) => {
                    self.note(&format!("無法讀取帳號與額度門檻，語意判斷無法使用：{error}"));
                    None
                }
            };
            let Some(bundle_id) = new_prefixed_id("evb") else {
                return self.note("無法取得系統亂數，驗證稍後重試");
            };
            let judge_profile_id = judge_account.as_ref().map(|a| a.profile_id.clone());
            let (attempt, previous) =
                match self.store.begin_verification_attempt(request_id, judge_profile_id, self.ports.clock.now()).await {
                    Ok(Some(begun)) => begun,
                    Ok(None) => return,
                    Err(error) => return self.note(&format!("驗證要求 {request_id} 無法開始新的嘗試：{error}")),
                };
            let context = crate::verification_adapter::ExecutionContext {
                ports: &self.ports,
                data_dir: &self.data_dir,
                artifacts_root: &self.artifacts_root,
                judge_account,
                store: &self.store,
                attempt: &attempt,
                previous: &previous,
            };
            match crate::verification_adapter::execute(context, item, bundle_id).await {
                crate::verification_adapter::AttemptOutcome::Completed(bundle) => {
                    let verdict = bundle.verdict;
                    match self.store.record_evidence_bundle(*bundle).await {
                        Ok(true) => self.note(&format!("驗證要求 {request_id} 完成：{}", verdict.as_str())),
                        Ok(false) => self.note(&format!("驗證要求 {request_id} 已經完成過，這次的結果不保存")),
                        Err(error) => self.note(&format!("驗證要求 {request_id} 的 Evidence Bundle 沒有保存：{error}")),
                    }
                    return;
                }
                crate::verification_adapter::AttemptOutcome::HandedOff { reason } => {
                    let now = self.ports.clock.now();
                    match self.store.hand_off_verification_attempt(request_id, attempt.attempt, reason, now).await {
                        Ok(true) => self.note(&format!("驗證要求 {request_id} 的第 {} 次嘗試交接（{reason}）", attempt.attempt)),
                        // 交接沒有記下：那次嘗試仍是執行中，下一次 tick 把它記為中斷再由新的嘗試接手
                        Ok(false) => return,
                        Err(error) => return self.note(&format!("驗證要求 {request_id} 的交接沒有記下：{error}")),
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ 整合

    /// 依序處理單一整合佇列，直到沒有可處理的項目；呼叫端負責持有 `verify_lock`（與驗證同一個工作，一次一個）。
    /// 同一個項目沒有前進（例如寫入失敗）就停下來，下一次 tick 再試，不會空轉。
    async fn integrate_pending(&self) {
        let mut last: Option<(String, i64)> = None;
        loop {
            let work = match self.store.next_integration().await {
                Ok(Some(work)) => work,
                Ok(None) => return,
                Err(error) => return self.note(&format!("無法讀取整合佇列：{error}")),
            };
            let key = (work.entry.id.clone(), work.entry.version);
            if last.as_ref() == Some(&key) {
                return;
            }
            last = Some(key);
            self.integrate(work).await;
        }
    }

    /// 處理一個整合項目到結束（或到某一步失敗為止）。
    async fn integrate(&self, work: IntegrationWork) {
        let id = work.entry.id.clone();
        let repo = std::path::PathBuf::from(&work.workspace.path);
        let mut entry = work.entry.clone();
        if entry.status != IntegrationStatus::Advancing {
            // 1. 先持久化「開始合併」；Task 已不在驗證中時項目直接以失敗結束
            let Some(begun) = self.apply_integration(IntegrationCommand::BeginMerge { id: id.clone() }).await else { return };
            if begun.status != IntegrationStatus::Merging {
                return;
            }
            // 2. 在整合目錄合併（目標分支不動）
            let dir = integration::integration_dir(&work.workspace);
            let merged = self
                .ports
                .workspace_fs
                .merge_for_integration(&repo, &begun.target_branch, &begun.branch, std::path::Path::new(&dir))
                .await;
            let result = match merged {
                Ok(MergeOutcome::Merged { commit, target_tip }) => MergeResult::Merged { commit, target_tip },
                Ok(MergeOutcome::Conflict { detail }) => MergeResult::Conflict { detail },
                Err(error) => MergeResult::Error { reason: error.to_string() },
            };
            let Some(recorded) = self.apply_integration(IntegrationCommand::RecordMerge { id: id.clone(), result }).await
            else {
                return;
            };
            if recorded.status != IntegrationStatus::Verifying {
                return;
            }
            // 3. 整合驗證：同一份確定性檢查，在合併後的整合目錄中執行
            let subject = crate::verifier::Subject { working_dir: &dir, manifest: &work.manifest, artifacts: &work.artifacts };
            let results = crate::verifier::run_checks_in(&self.ports, &self.artifacts_root, &work.checks, &subject).await;
            let Some(checked) = self.apply_integration(IntegrationCommand::RecordChecks { id: id.clone(), results }).await
            else {
                return;
            };
            entry = checked;
        }
        if entry.status != IntegrationStatus::Advancing {
            return;
        }
        // 4. 驗證通過：把目標分支前移到合併結果（冪等；崩潰後以同一個結果重試）
        let (Some(commit), Some(tip)) = (entry.merge_commit.clone(), entry.target_tip.clone()) else { return };
        let advanced = self.ports.workspace_fs.advance_branch(&repo, &entry.target_branch, &tip, &commit).await;
        let result = match advanced {
            Ok(AdvanceOutcome::Advanced) => AdvanceResult::Advanced,
            Ok(AdvanceOutcome::TargetMoved { actual }) => AdvanceResult::TargetMoved { actual },
            Ok(AdvanceOutcome::Blocked { detail }) => AdvanceResult::Blocked { detail },
            Err(error) => AdvanceResult::Error { reason: error.to_string() },
        };
        self.apply_integration(IntegrationCommand::RecordAdvance { id, result }).await;
    }

    /// 套用一個整合命令並記錄結果；被拒絕或寫入失敗只記日誌（下一次 tick 以最新狀態重試）。
    async fn apply_integration(&self, command: IntegrationCommand) -> Option<baton_domain::integration::IntegrationEntry> {
        match self.store.apply_integration(command, self.ports.clock.now()).await {
            Ok(change) => {
                let entry = change.entry;
                match (entry.status, &entry.failure) {
                    (IntegrationStatus::Failed, Some(reason)) => {
                        self.note(&format!("Task {} 未通過整合：{reason}", entry.task_id))
                    }
                    (IntegrationStatus::Integrated, _) => self.note(&format!(
                        "Task {} 已整合到 {}（{}），Task 完成",
                        entry.task_id,
                        entry.target_branch,
                        entry.merge_commit.as_deref().unwrap_or("")
                    )),
                    _ => {}
                }
                Some(entry)
            }
            Err(error) => {
                self.note(&format!("整合項目的命令沒有套用：{error}"));
                None
            }
        }
    }

    // ------------------------------------------------------------ 清理

    /// 把符合條件的 worktree 登記為待清理，再處理到期的清理；呼叫端負責持有 `verify_lock`（與驗證、整合同一個工作）。
    async fn clean_pending(&self) {
        let now = self.ports.clock.now();
        match self.store.request_cleanups(now).await {
            Ok(requested) => {
                for cleanup in requested {
                    self.note(&format!("Task {} 已完成並整合，worktree {} 登記為待清理", cleanup.task_id, cleanup.path));
                }
            }
            Err(error) => return self.note(&format!("無法登記待清理的 worktree：{error}")),
        }
        let due = match self.store.due_cleanups(now).await {
            Ok(due) => due,
            Err(error) => return self.note(&format!("無法讀取待清理的 worktree：{error}")),
        };
        for work in due {
            self.clean(work).await;
        }
    }

    /// 一次清理嘗試：先持久化「清理中」，重新核對安全條件，符合才移除，再記錄結果。
    async fn clean(&self, work: CleanupWork) {
        let task_id = work.cleanup.task_id.clone();
        let begin = CleanupCommand::Begin { task_id: task_id.clone() };
        let begun = match self.store.apply_cleanup(begin, self.ports.clock.now()).await {
            Ok(change) => change.cleanup,
            Err(error) => return self.note(&format!("Task {task_id} 的清理沒有開始：{error}")),
        };
        let repo = PathBuf::from(&work.workspace.path);
        let target = PathBuf::from(&begun.path);
        let result = match self.ports.workspace_fs.inspect_worktree(&repo, &target).await {
            Err(error) => Err(format!("無法核對 worktree {}：{error}", begun.path)),
            Ok(state) => match cleanup::check_safety(&begun, &inspection_of(state)) {
                Err(reason) => Err(reason),
                Ok(()) => {
                    let trash = cleanup::trash_dir(&work.workspace, &task_id);
                    self.ports
                        .workspace_fs
                        .remove_worktree(&repo, &target, std::path::Path::new(&trash))
                        .await
                        .map_err(|error| format!("無法移除 worktree {}：{error}", begun.path))
                }
            },
        };
        let finish = CleanupCommand::Finish { task_id: task_id.clone(), result };
        match self.store.apply_cleanup(finish, self.ports.clock.now()).await {
            Ok(change) => {
                let cleanup = change.cleanup;
                match (cleanup.status, &cleanup.failure) {
                    (CleanupStatus::Cleaned, _) => self.note(&format!("Task {task_id} 的 worktree {} 已清理", cleanup.path)),
                    (CleanupStatus::Failed, Some(reason)) => {
                        self.note(&format!("Task {task_id} 的 worktree 清理失敗，不再自動重試：{reason}"))
                    }
                    (_, Some(reason)) => self.note(&format!("Task {task_id} 的 worktree 這次沒有清理，稍後重試：{reason}")),
                    _ => {}
                }
            }
            Err(error) => self.note(&format!("Task {task_id} 的清理結果沒有記錄：{error}")),
        }
    }

    // ------------------------------------------------------------ 取消

    async fn process_cancellations(&self) {
        let executing = match self.store.executing_runs().await {
            Ok(executing) => executing,
            Err(error) => return self.note(&format!("無法讀取執行中的 Run：{error}")),
        };
        // 所有被取消的 Run 同時停止（各自有寬限期）：一個停得慢的 Run 不會讓其他取消排在後面等
        let mut stopping = FuturesUnordered::new();
        for item in executing {
            let run = item.view.run.clone();
            let (true, Some(process)) = (run.cancel_requested_at.is_some(), run.process.as_ref()) else { continue };
            self.ensure_managed(&item).await;
            let identity = identity_of(process);
            let process_control = self.ports.process_control.clone();
            let grace = self.stop_grace;
            stopping.push(async move {
                let result = process_control.stop(&identity, grace).await;
                (run, result)
            });
        }
        // 最長要等寬限期加確認上限；完全退出開始時就不再等，讓出 tick 鎖，還沒停止的 Run 改由停止流程處理
        let mut exiting = self.exiting.subscribe();
        loop {
            let next = tokio::select! {
                next = stopping.next() => next,
                _ = exiting.wait_for(|exiting| *exiting) => {
                    return self.note(&format!("完全退出開始：{} 個 Run 的取消改由停止流程處理", stopping.len()));
                }
            };
            let Some((run, result)) = next else { return };
            match result {
                Ok(StopResult::StoppedGracefully | StopResult::ForceKilled) => {
                    self.streams.lock().await.remove(&run.id);
                    self.apply(&run, RunCommand::ConfirmCancelled { run_id: run.id.clone() }).await;
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
            // 根程序不在不代表整個程序樹都結束了：殘留的子孫程序清掉之前不下結論，避免交出寫入所有權後出現第二個 writer
            let observed = match observed {
                ProcessObservation::Dead if !self.release_process_tree(&item, Duration::ZERO).await => {
                    ProcessObservation::Unknown
                }
                other => other,
            };
            self.apply(&item.view.run, RunCommand::ReconcileLease { run_id: item.view.run.id.clone(), observed }).await;
        }
    }

    /// Run 結束前收尾它的程序樹（D-10）：等 `grace` 內自行結束，否則終止 Job 內殘留的程序。
    /// 回傳 false 表示無法確認整個程序樹都已結束，呼叫端不能結束 Run。
    async fn release_process_tree(&self, item: &ExecutingRun, grace: Duration) -> bool {
        let run = &item.view.run;
        let Some(process) = run.process.as_ref() else { return true };
        self.ensure_managed(item).await;
        match self.ports.process_control.release(&identity_of(process), grace).await {
            Ok(StopResult::StoppedGracefully | StopResult::ForceKilled) => true,
            Ok(StopResult::StillAlive) => {
                self.note(&format!("Run {} 的程序樹裡還有程序沒有結束，保留寫入所有權並稍後重試", run.id));
                false
            }
            Err(error) => {
                self.note(&format!("Run {} 無法確認程序樹已結束，保留寫入所有權並稍後重試：{error}", run.id));
                false
            }
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

/// 把 WorkspaceFs 核對到的狀態轉成 Domain 的型別。
fn inspection_of(state: WorktreeState) -> Inspection {
    match state {
        WorktreeState::Missing => Inspection::Missing,
        WorktreeState::Registered { branch, unsaved } => Inspection::Registered { branch, unsaved },
        WorktreeState::Foreign => Inspection::Foreign,
    }
}

fn affected_of(run: &Run, disposition: RunDisposition, message: Option<String>) -> AffectedRun {
    AffectedRun {
        run_id: run.id.clone(),
        workspace_id: run.workspace_id.clone(),
        task_id: run.task_id.clone(),
        disposition,
        message,
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

pub(crate) fn new_prefixed_id(prefix: &str) -> Option<String> {
    let mut entropy = [0u8; 12];
    getrandom::fill(&mut entropy).ok()?;
    let hex: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("{prefix}_{hex}"))
}
