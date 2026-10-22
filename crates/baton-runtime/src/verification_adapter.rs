//! Verification Adapter（票 45、46，spec D-29）：依 Verification Profile 執行一次驗證並產生 Evidence Bundle。
//!
//! 操作與判定分離：這裡只經由 Port 執行操作（`VerificationDriver` 的命令與 HTTP、`BrowserDriver` 的瀏覽器操作）、
//! 呼叫受限的語意 judge（`SemanticJudge`），把結果轉成 Domain 的 `Observation`，判定一律交給
//! `baton_domain::verification_profile`。順序：
//! 準備本次專屬的狀態目錄 → build 識別 → doctor → 啟動服務 → 就緒（重試）→ 資料準備 → 旅程 → teardown → 停止服務 →
//! 移除狀態目錄。前置階段受阻時旅程不執行；旅程中一步沒有通過，之後的步驟不執行；teardown 與清理一定執行，
//! 清理只碰本次驗證的狀態目錄與它啟動的服務，證據保存在回傳的 Bundle。控制資料庫只寫入這次嘗試的驗證日誌。
//!
//! web（票 46）：
//! - 每條旅程一個隔離的瀏覽器 Session；旅程綁定登入身分時，先執行該身分的登入步驟，再以 `check` 核對實際登入的身分，
//!   都通過才執行旅程步驟。persona 只作為 judge 的領域背景，不決定以誰登入。
//! - 秘密：開始時解析 Profile 宣告的參照，值只在交給 Runner 的操作中代入；證據的文字、judge 的輸入都以值遮蔽，
//!   證據檔案含有值或可能記錄了輸入的秘密時不保存。
//! - 語意 judge：輸入只有當前畫面與 Profile 中的固定預期；模型限流之後同一次嘗試不再呼叫模型。
//!
//! 已驗證動作重播（票 48）：
//! - 目標導向動作（`act`）先查這個案例的錄製：身分一致、狀態 valid、不是正式環境（或步驟個別允許）時重播錄製的動作，
//!   仍實際執行並重新檢查後置條件；動作無法完成或後置條件明確不成立時把錄製標為 stale。身分不一致時標為 stale、不沿用。
//! - 沒有可用的錄製時由模型依當前畫面規劃一個具體動作（只能點擊、輸入或按鍵）；步驟通過而且有不靠模型的斷言成立，
//!   才把這個動作錄製下來。錄製的變更隨 Bundle 一起保存。
//! - `no_live_model`：這次驗證的模型 Port 換成一律拒絕的實作，而且每個需要模型的地方在呼叫前就擋下（記為
//!   `live_model_disabled`，計入 `blocked_model_calls`），所以不會有任何即時模型呼叫。
//! - 用量分開計算：實際的模型呼叫、模型決定的動作、重播的動作與被擋下的模型呼叫。
//!
//! 嘗試與交接（票 49，規則在 `baton_domain::verification_attempt`）：
//! - 每個完成的步驟與判定、build、啟動的服務與瀏覽器 Session（只有識別）、證據檔案、完成的旅程都依序寫入這次嘗試的日誌；
//!   外部操作（`external` 的旅程步驟）在送出前寫入，寫不進去就不送出。
//! - 語意 judge 限流而另有帳號可用（額度門檻 open、這個要求中沒有限流過）時，在那個步驟之後停止旅程，照常 teardown、
//!   停止服務、移除狀態目錄，回傳 `AttemptOutcome::HandedOff`，由 Supervisor 以另一個帳號開始新的嘗試。
//!   沒有其他帳號時留在原地繼續，之後的語意判斷不呼叫模型（只執行不需模型的斷言）。
//! - 承接的嘗試先以日誌中的參照停止前一次留下的服務與瀏覽器 Session，無法確認停止時不執行任何操作
//!   （也不準備或移除狀態目錄、不執行 teardown：舊程序可能仍在使用它們）；
//!   旅程依 `plan_journey` 沿用、從新的 fixture 整條重跑，或不重送已開始的外部操作。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use baton_domain::artifact::sha256_hex;
use baton_domain::verification_profile::replay::{
    self, ActionRecord, ActionSource, CacheState, RecordedAction, ReplayDecision, ReplayEntry, VerificationUsage,
};
use baton_domain::handoff::GateState;
use baton_domain::verification_attempt::{self as va, Attempt, AttemptStatus, JournalEntry, JourneyPlan, Takeover};
use baton_domain::verification_profile::{
    self as vp, Assertion, BrowserAction, CleanupRecord, EvidenceArtifact, EvidenceArtifactKind, EvidenceBundle,
    EvidencePolicy, Interface, Journey, Judgement, Observation, Operation, Phase, ProfileDefinition, ScreenshotPolicy,
    Step, StepRecord, Verdict,
};
use baton_ports::{
    ActionPlan, ActionRequest, BrowserCommand, BrowserOutcome, BrowserSession, BrowserSessionRequest, BrowserStep,
    CommandOutput, CommandRequest, HttpOutcome, HttpRequest, JudgeAccount, JudgeOutcome, JudgeRequest, PageSnapshot,
    PortResult, Ports, SemanticJudge, ServiceHandle, ServiceRequest,
};
use baton_store::{PendingVerificationRun, Store, StoreResult};

/// 本次驗證專屬的狀態目錄：`<資料目錄>\verification\<驗證要求>`。
pub fn state_dir(data_dir: &Path, request_id: &str) -> PathBuf {
    data_dir.join("verification").join(request_id)
}

/// 執行驗證需要的環境（Runtime 提供）。
pub struct ExecutionContext<'a> {
    pub ports: &'a Ports,
    pub data_dir: &'a Path,
    /// 成果儲存的根目錄；證據檔案寫在其下的 `verification\`。
    pub artifacts_root: &'a Path,
    /// 模型呼叫（語意 judge 與動作規劃）使用的帳號；沒有可用帳號時為 `None`（需要模型的斷言與動作無法判定）。
    pub judge_account: Option<JudgeAccount>,
    /// 控制資料庫：寫入這次嘗試的日誌、判斷有沒有承接帳號（票 49）。
    pub store: &'a Store,
    /// 這次嘗試（已在控制資料庫開始）。
    pub attempt: &'a Attempt,
    /// 同一個要求先前的嘗試與日誌（依嘗試順序）。
    pub previous: &'a [Attempt],
}

/// 一次嘗試的結果。
#[derive(Debug)]
pub enum AttemptOutcome {
    Completed(Box<EvidenceBundle>),
    /// 在步驟邊界交接：這次的測試程序已停止，由新的嘗試接手。
    HandedOff { reason: &'static str },
}

/// 語意 judge 可以使用的帳號（票 49）：可派工、額度門檻 open（spec D-13；paused 與 draining 都不再接新的模型呼叫），
/// 而且不在 `excluded`（這個要求中限流過的帳號）。依帳號的建立順序。
pub async fn judge_accounts(store: &Store, excluded: &[String]) -> StoreResult<Vec<JudgeAccount>> {
    let profiles = store.list_profiles().await?;
    let gates = store.quota_gates().await?;
    Ok(baton_domain::scheduler::usable_profiles(&profiles)
        .into_iter()
        .filter(|p| !excluded.contains(&p.id))
        .filter(|p| gates.get(&p.id).is_none_or(|g| g.state == GateState::Open))
        .map(|p| JudgeAccount { profile_id: p.id.clone(), config_dir: PathBuf::from(&p.config_dir) })
        .collect())
}

/// `no_live_model` 時取代模型 Port 的實作：一律拒絕，不呼叫任何模型（票 48）。
struct NoLiveModel;

const NO_LIVE_MODEL: &str = "no_live_model：這次驗證禁止即時模型呼叫";

#[async_trait]
impl SemanticJudge for NoLiveModel {
    async fn judge(&self, _request: JudgeRequest) -> PortResult<JudgeOutcome> {
        Ok(JudgeOutcome::Unavailable { reason: NO_LIVE_MODEL.into() })
    }

    async fn plan(&self, _request: ActionRequest) -> PortResult<ActionPlan> {
        Ok(ActionPlan::Unavailable { reason: NO_LIVE_MODEL.into() })
    }
}

/// 最近一次觀察到的畫面（動作規劃的輸入）。
#[derive(Default)]
struct PageView {
    url: String,
    title: String,
    text: String,
}

/// 目前旅程的瀏覽器 Session。
struct Browser {
    session: BrowserSession,
    journey: String,
    /// 這個 Session 輸入過秘密：trace 與錄影不保存。
    secret_entered: bool,
    /// 這個 Session 的畫面顯示過秘密：trace 與錄影不保存。
    secret_shown: bool,
    /// 截圖時遮住的 selector（政策指定的，加上輸入過秘密的欄位）。
    mask: Vec<String>,
    /// Runner 回報的引擎（錄製身分的一部分）。
    engine: String,
    /// 最近一次觀察到的畫面。
    page: PageView,
}

/// 執行中的一次驗證。
struct Execution<'a> {
    ports: &'a Ports,
    definition: &'a ProfileDefinition,
    /// 證據政策；遮蔽清單已加入本次解析到的秘密值。
    policy: EvidencePolicy,
    subject_dir: PathBuf,
    run_id: &'a str,
    state_dir: String,
    artifacts_root: PathBuf,
    /// 剩餘的操作次數（不含 teardown）。
    budget: u32,
    /// 剩餘的模型呼叫次數（語意判斷與動作規劃共用）。
    model_budget: u32,
    judge_account: Option<JudgeAccount>,
    /// 這次嘗試中模型已經限流：之後的語意判斷與動作規劃不再呼叫模型。
    judge_rate_limited: bool,
    /// 模型 Port；`no_live_model` 時是一律拒絕的實作。
    model: Arc<dyn SemanticJudge>,
    /// 禁止即時模型呼叫（票 48）。
    no_live_model: bool,
    /// 這個 Workspace 目標導向動作的錄製。
    replay: &'a [ReplayEntry],
    actions: Vec<ActionRecord>,
    usage: VerificationUsage,
    store: &'a Store,
    attempt: u32,
    /// 這個要求中限流過的帳號（不能承接）。
    excluded: Vec<String>,
    /// 要交接（原因）：目前的步驟之後停止旅程。
    handoff: Option<&'static str>,
    /// 這次嘗試寫入的日誌（Bundle 的嘗試摘要使用）。
    journal: Vec<JournalEntry>,
    /// 已解析的秘密 (識別, 值)；只在交給 Runner 的操作中代入。
    secrets: Vec<(String, String)>,
    /// 解析不到的秘密識別。
    unresolved: Vec<String>,
    browser: Option<Browser>,
    /// 目前旅程的 persona 說明（judge 的領域背景）。
    persona: Option<String>,
    steps: Vec<StepRecord>,
    artifacts: Vec<EvidenceArtifact>,
    problems: Vec<String>,
    /// 前置階段已經受阻：之後不再執行任何操作（teardown 除外）。
    blocked: bool,
}

/// 瀏覽器步驟的截圖。
struct Screenshot {
    bytes: Vec<u8>,
    /// 截圖時的畫面顯示了秘密：影像中的文字無法從位元組找到，以畫面文字判斷。
    shows_secret: bool,
}

/// 畫面（網址、標題、文字）是否顯示了任何要遮蔽的值。
fn shows_secret(page: &PageSnapshot, policy: &EvidencePolicy) -> bool {
    [&page.url, &page.title, &page.text].iter().any(|t| redact(t, policy) != **t)
}

/// 只遮蔽、不截斷（judge 的輸入需要完整的畫面文字）。
fn redact(text: &str, policy: &EvidencePolicy) -> String {
    let mut text = text.to_string();
    for secret in policy.redact.iter().filter(|s| !s.is_empty()) {
        text = text.replace(secret.as_str(), vp::REDACTED);
    }
    text
}

/// 具體的瀏覽器操作；目標導向動作沒有對應的操作（由 `run_act` 先規劃或取出錄製）。
fn browser_command(action: &BrowserAction) -> Option<BrowserCommand> {
    Some(match action.clone() {
        BrowserAction::Goto { url } => BrowserCommand::Goto { url },
        BrowserAction::Click { selector } => BrowserCommand::Click { selector },
        BrowserAction::Fill { selector, text } => BrowserCommand::Fill { selector, text },
        BrowserAction::Press { selector, key } => BrowserCommand::Press { selector, key },
        BrowserAction::WaitFor { selector } => BrowserCommand::WaitFor { selector },
        BrowserAction::Act { .. } => return None,
    })
}

/// 模型規劃的操作轉成可錄製的動作。
fn planned_action(command: &BrowserCommand) -> BrowserAction {
    match command.clone() {
        BrowserCommand::Goto { url } => BrowserAction::Goto { url },
        BrowserCommand::Click { selector } => BrowserAction::Click { selector },
        BrowserCommand::Fill { selector, text } => BrowserAction::Fill { selector, text },
        BrowserCommand::Press { selector, key } => BrowserAction::Press { selector, key },
        BrowserCommand::WaitFor { selector } => BrowserAction::WaitFor { selector },
    }
}

/// 動作的一行描述（證據中保存遮蔽後的版本）。
fn action_description(action: &BrowserAction) -> String {
    Operation::Browser { action: action.clone(), timeout_ms: None }.describe()
}

/// 沒有執行操作的步驟紀錄，附上說明與來源代碼。
fn not_run_with(
    phase: Phase,
    journey: &str,
    step: &Step,
    (verdict, reason_code, engine_code): (Verdict, &str, Option<&str>),
    output: &str,
    policy: &EvidencePolicy,
) -> StepRecord {
    let mut record = vp::not_run(phase, Some(journey), step, verdict, reason_code, policy);
    record.engine_code = engine_code.map(str::to_string);
    record.output = vp::sanitize(output, policy);
    record
}

impl Execution<'_> {
    /// 執行操作並觀察結果；`operation` 已代換佔位字，秘密在這裡才代入。瀏覽器步驟另外回傳截圖。
    async fn observe(&mut self, operation: &Operation, assertions: &[Assertion]) -> (Observation, Option<Screenshot>) {
        let timeout = Duration::from_millis(operation.effective_timeout_ms());
        let revealed = vp::reveal_secrets(operation, &self.secrets);
        let driver = &self.ports.verification_driver;
        let observation = match revealed {
            Operation::Command { program, args, .. } => {
                let request = CommandRequest { program, args, working_dir: self.subject_dir.clone(), timeout };
                match driver.run_command(request).await {
                    Ok(CommandOutput::Exited { code, output }) => Observation::Exited { code, output },
                    Ok(CommandOutput::Terminated { output }) => Observation::Terminated { output },
                    Ok(CommandOutput::TimedOut { output }) => Observation::TimedOut { output },
                    Err(error) => Observation::NotStarted { reason: error.to_string() },
                }
            }
            Operation::Http { method, url, headers, body, .. } => {
                let headers = headers.into_iter().map(|h| (h.name, h.value)).collect();
                match driver.send_http(HttpRequest { method, url, headers, body, timeout }).await {
                    Ok(HttpOutcome::Responded { status, body }) => Observation::Responded { status, body },
                    Ok(HttpOutcome::Unreachable { reason }) => Observation::Unreachable { reason },
                    Ok(HttpOutcome::TimedOut) => Observation::TimedOut { output: String::new() },
                    Err(error) => Observation::Unreachable { reason: error.to_string() },
                }
            }
            Operation::Browser { action, .. } => {
                let Some(command) = browser_command(&action) else {
                    return (Observation::Unsupported { reason: "目標導向動作需要先規劃或重播".into() }, None);
                };
                let entered_secret = !operation.secret_keys().is_empty();
                return self.observe_browser(command, entered_secret, operation.filled_selector(), timeout, assertions).await;
            }
            Operation::Manual { instruction } => {
                Observation::Unsupported { reason: format!("Adapter 無法自動執行人工步驟：{instruction}") }
            }
        };
        (observation, None)
    }

    /// 執行瀏覽器操作，觀察當前畫面，再以畫面做語意判斷。畫面顯示秘密時，這個 Session 標記為顯示過秘密。
    async fn observe_browser(
        &mut self,
        command: BrowserCommand,
        entered_secret: bool,
        filled: Option<&str>,
        timeout: Duration,
        assertions: &[Assertion],
    ) -> (Observation, Option<Screenshot>) {
        let screenshots = self.policy.screenshots != ScreenshotPolicy::Off;
        let Some(browser) = self.browser.as_mut() else {
            return (Observation::Unsupported { reason: "這個步驟沒有瀏覽器 Session".into() }, None);
        };
        if entered_secret {
            // 輸入秘密的欄位：之後的截圖都遮住，trace 與錄影不保存
            browser.secret_entered = true;
            if let Some(selector) = filled.filter(|s| !browser.mask.iter().any(|m| m == s)) {
                browser.mask.push(selector.to_string());
            }
        }
        let visible: Vec<String> = assertions
            .iter()
            .filter_map(|a| match a {
                Assertion::Visible { selector } => Some(selector.clone()),
                _ => None,
            })
            .collect();
        let step = BrowserStep {
            command,
            timeout,
            visible: visible.clone(),
            screenshot_mask: screenshots.then(|| browser.mask.clone()),
        };
        let session = browser.session.clone();
        match self.ports.browser_driver.step(&session, step).await {
            Ok(BrowserOutcome::Done { mut page }) => {
                self.remember(&page);
                let screenshot = self.screenshot(&mut page);
                let judgements = self.judge(assertions, &page).await;
                let observation = Observation::Page {
                    url: page.url,
                    status: page.status,
                    text: page.text,
                    visible: visible.into_iter().zip(page.visible).collect(),
                    judgements,
                };
                (observation, screenshot)
            }
            Ok(BrowserOutcome::ActionFailed { error, message, page }) => {
                if let Some(page) = &page {
                    self.remember(page);
                }
                let screenshot = page.and_then(|mut p| self.screenshot(&mut p));
                (Observation::ActionFailed { error, message }, screenshot)
            }
            Ok(BrowserOutcome::Unreachable { reason }) => (Observation::Unreachable { reason }, None),
            Ok(BrowserOutcome::TimedOut) => (Observation::TimedOut { output: String::new() }, None),
            Err(error) => (Observation::ActionFailed { error: "RunnerError".into(), message: error.to_string() }, None),
        }
    }

    /// 記下目前 Session 最近一次的畫面（已遮蔽秘密），作為之後動作規劃的輸入。
    fn remember(&mut self, page: &PageSnapshot) {
        let view = PageView {
            url: redact(&page.url, &self.policy),
            title: redact(&page.title, &self.policy),
            text: redact(&page.text, &self.policy).chars().take(vp::JUDGE_PAGE_TEXT_MAX_CHARS).collect(),
        };
        if let Some(browser) = self.browser.as_mut() {
            browser.page = view;
        }
    }

    /// 取出畫面的截圖；畫面顯示秘密時把目前的 Session 標記為顯示過秘密（即使不截圖）。
    fn screenshot(&mut self, page: &mut PageSnapshot) -> Option<Screenshot> {
        let shows_secret = shows_secret(page, &self.policy);
        if let Some(browser) = self.browser.as_mut().filter(|_| shows_secret) {
            browser.secret_shown = true;
        }
        page.screenshot.take().map(|bytes| Screenshot { bytes, shows_secret })
    }

    /// 依序處理步驟中的語意判斷斷言。輸入只有當前畫面（已遮蔽秘密）、Profile 的固定預期與 persona 說明。
    async fn judge(&mut self, assertions: &[Assertion], page: &PageSnapshot) -> Vec<Judgement> {
        let mut judgements = Vec::new();
        for assertion in assertions {
            let Assertion::Judge { expectation } = assertion else { continue };
            let judgement = if self.no_live_model {
                self.usage.blocked_model_calls += 1;
                Judgement::LiveModelDisabled
            } else if self.judge_rate_limited {
                Judgement::RateLimited
            } else if self.model_budget == 0 {
                Judgement::BudgetExhausted
            } else if let Some(account) = self.judge_account.clone() {
                let profile_id = account.profile_id.clone();
                self.model_budget -= 1;
                self.usage.live_model_calls += 1;
                let text: String = redact(&page.text, &self.policy).chars().take(vp::JUDGE_PAGE_TEXT_MAX_CHARS).collect();
                let request = JudgeRequest {
                    account,
                    expectation: expectation.clone(),
                    persona: self.persona.clone(),
                    page_url: redact(&page.url, &self.policy),
                    page_title: redact(&page.title, &self.policy),
                    page_text: text,
                };
                match self.model.judge(request).await {
                    Ok(JudgeOutcome::Decided { satisfied, rationale }) => Judgement::Decided { satisfied, rationale },
                    Ok(JudgeOutcome::RateLimited { .. }) => {
                        self.judge_rate_limited = true;
                        self.hand_off_if_possible(profile_id).await;
                        Judgement::RateLimited
                    }
                    Ok(JudgeOutcome::Unavailable { reason }) => Judgement::Unavailable { reason },
                    Err(error) => Judgement::Unavailable { reason: error.to_string() },
                }
            } else {
                Judgement::Unavailable { reason: "沒有可用的帳號執行語意判斷".into() }
            };
            judgements.push(judgement);
        }
        judgements
    }

    fn bind(&self, operation: &Operation) -> Operation {
        vp::bind_operation(operation, self.run_id, &self.state_dir)
    }

    /// 執行並判定一個步驟；需要操作而預算已經用盡時判定為 `budget_exhausted`，引用的秘密解析不到時是 `environment_blocked`。
    /// 人工步驟不消耗預算。
    async fn run_step(&mut self, phase: Phase, journey: Option<&str>, step: &Step) -> Verdict {
        let operation = self.bind(&step.operation);
        if operation.secret_keys().iter().any(|k| self.unresolved.contains(k)) {
            return self.skip(phase, journey, step, Verdict::EnvironmentBlocked, "secret_unavailable").await;
        }
        let automated = !matches!(operation, Operation::Manual { .. });
        if automated && self.budget == 0 {
            return self.skip(phase, journey, step, Verdict::BudgetExhausted, "operation_budget_exhausted").await;
        }
        // 外部操作：送出前先寫入日誌；寫不進去就不送出（之後無法判斷送過沒有）
        if let (true, Some(key)) = (step.external, journey) {
            let entry = JournalEntry::ExternalStarted { journey: key.to_string(), step: step.key.clone() };
            if !self.record(entry).await {
                return self.skip(phase, journey, step, Verdict::Inconclusive, va::REASON_JOURNAL_UNAVAILABLE).await;
            }
        }
        if let (true, Some(journey)) = (operation.is_act(), journey) {
            return self.run_act(phase, journey, step, &operation).await;
        }
        if automated {
            self.budget -= 1;
        }
        let (observation, screenshot) = self.observe(&operation, &step.assertions).await;
        let record = vp::judge_step(phase, journey, step, &operation, observation, 1, &self.policy);
        self.keep_screenshot(journey, step, &record, screenshot).await;
        self.push(record).await
    }

    /// 依政策保存步驟的截圖。
    async fn keep_screenshot(&mut self, journey: Option<&str>, step: &Step, record: &StepRecord, screenshot: Option<Screenshot>) {
        if let Some(Screenshot { bytes, shows_secret }) = screenshot {
            if self.policy.screenshots == ScreenshotPolicy::EveryStep || record.verdict != Verdict::Passed {
                let name = format!("{}-{}.png", journey.unwrap_or("profile"), step.key);
                self.store_artifact(EvidenceArtifactKind::Screenshot, journey, Some(&step.key), name, bytes, shows_secret).await;
            }
        }
    }

    /// 目標導向動作（票 48）：有可用的錄製時重播，否則由模型規劃；`operation` 已代換佔位字。操作預算已確認足夠。
    async fn run_act(&mut self, phase: Phase, journey: &str, step: &Step, operation: &Operation) -> Verdict {
        let definition = self.definition;
        let Operation::Browser { action: BrowserAction::Act { goal, params, replay_in_production }, .. } = operation else {
            unreachable!("run_act 只處理目標導向動作");
        };
        let engine = self.browser.as_ref().map_or(replay::UNKNOWN_ENGINE.to_string(), |b| b.engine.clone());
        let identity = definition
            .journey(journey)
            .and_then(|j| replay::identity(definition, j, step, &engine))
            .expect("目標導向步驟屬於這條旅程");
        let case_key = replay::case_key(journey, &step.key);
        let entry = self.replay.iter().find(|e| e.case_key == case_key);
        let mut action_record = ActionRecord {
            journey: journey.to_string(),
            step: step.key.clone(),
            goal: vp::sanitize(&identity.goal, &self.policy),
            cache: CacheState::None,
            source: None,
            action: None,
            stale_reasons: Vec::new(),
            recorded: None,
        };
        let timeout = Duration::from_millis(operation.effective_timeout_ms());
        let decision = replay::decide(entry, &identity, definition.replay.production, *replay_in_production);
        let (action, source) = match decision {
            ReplayDecision::Replay(action) => {
                action_record.cache = CacheState::Valid;
                self.usage.replayed_actions += 1;
                (action, ActionSource::Replay)
            }
            ReplayDecision::Live { cache, stale } => {
                action_record.cache = cache;
                action_record.stale_reasons = stale;
                match self.plan(goal, params).await {
                    Ok(action) => {
                        self.usage.live_actions += 1;
                        (action, ActionSource::Live)
                    }
                    Err((verdict, output)) => {
                        let record = not_run_with(phase, journey, step, verdict, &output, &self.policy);
                        self.actions.push(action_record);
                        return self.push(record).await;
                    }
                }
            }
        };
        self.budget -= 1;
        let description = action_description(&action);
        let shown = vp::sanitize(&description, &self.policy);
        action_record.source = Some(source);
        action_record.action = Some(shown.clone());
        let command = browser_command(&action).expect("錄製與規劃的動作都是具體操作");
        let (observation, screenshot) = self.observe_browser(command, false, None, timeout, &step.assertions).await;
        let record = vp::judge_step(phase, Some(journey), step, operation, observation, 1, &self.policy);
        match source {
            ActionSource::Replay if replay::disproven(&record) => {
                action_record.stale_reasons.push("postcondition_failed".into());
            }
            // 只錄製被後續有效斷言證明、而且不含秘密的動作
            ActionSource::Live if replay::proven(&record) && shown == description => {
                action_record.recorded = Some(RecordedAction { identity, action });
            }
            _ => {}
        }
        let record = replay::settle(record, source);
        self.keep_screenshot(Some(journey), step, &record, screenshot).await;
        self.actions.push(action_record);
        self.push(record).await
    }

    /// 由模型依當前畫面為目標規劃一個具體動作；無法規劃時回傳 (判定, reason_code, engine_code) 與說明。
    #[allow(clippy::type_complexity)]
    async fn plan(
        &mut self,
        goal: &str,
        params: &std::collections::BTreeMap<String, String>,
    ) -> Result<BrowserAction, ((Verdict, &'static str, Option<&'static str>), String)> {
        if self.no_live_model {
            self.usage.blocked_model_calls += 1;
            let reason = "no_live_model：沒有可用的錄製，而且這次驗證不呼叫模型".to_string();
            return Err(((Verdict::Inconclusive, "live_model_disabled", Some("model:disabled")), reason));
        }
        if self.judge_rate_limited {
            return Err(((Verdict::Inconclusive, "planner_rate_limited", Some("model:rate_limited")), "模型限流，沒有規劃".into()));
        }
        if self.model_budget == 0 {
            let reason = "模型呼叫預算已用盡，沒有規劃".to_string();
            return Err(((Verdict::BudgetExhausted, "model_call_budget_exhausted", Some("model:budget_exhausted")), reason));
        }
        let unavailable = |reason: String| ((Verdict::Inconclusive, "planner_unavailable", Some("model:unavailable")), reason);
        let Some(account) = self.judge_account.clone() else {
            return Err(unavailable("沒有可用的帳號規劃動作".into()));
        };
        let profile_id = account.profile_id.clone();
        self.model_budget -= 1;
        self.usage.live_model_calls += 1;
        let page = self.browser.as_ref().map(|b| &b.page);
        let request = ActionRequest {
            account,
            goal: goal.to_string(),
            params: params.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            persona: self.persona.clone(),
            page_url: page.map(|p| p.url.clone()).unwrap_or_default(),
            page_title: page.map(|p| p.title.clone()).unwrap_or_default(),
            page_text: page.map(|p| p.text.clone()).unwrap_or_default(),
        };
        match self.model.plan(request).await {
            Ok(ActionPlan::Planned { command, .. }) => {
                let action = planned_action(&command);
                if replay::plannable(&action) {
                    Ok(action)
                } else {
                    Err(unavailable(format!("模型選了不允許的動作：{}", action_description(&action))))
                }
            }
            Ok(ActionPlan::Unachievable { rationale }) => {
                Err(((Verdict::Inconclusive, "act_not_achievable", Some("model:unachievable")), rationale))
            }
            Ok(ActionPlan::RateLimited { .. }) => {
                // 與語意 judge 相同（票 49）：另有帳號可以接手時，這個步驟之後交接
                self.judge_rate_limited = true;
                self.hand_off_if_possible(profile_id).await;
                Err(((Verdict::Inconclusive, "planner_rate_limited", Some("model:rate_limited")), "模型限流，沒有規劃".into()))
            }
            Ok(ActionPlan::Unavailable { reason }) => Err(unavailable(reason)),
            Err(error) => Err(unavailable(error.to_string())),
        }
    }

    /// 就緒條件：最多嘗試 `attempts` 次，兩次之間等待 `interval_ms`；只保存最後一次的觀察與總次數。
    async fn run_readiness(&mut self, readiness: &vp::Readiness) -> Verdict {
        let operation = self.bind(&readiness.step.operation);
        let mut attempts = 0;
        loop {
            if self.budget == 0 {
                return self
                    .skip(Phase::Readiness, None, &readiness.step, Verdict::BudgetExhausted, "operation_budget_exhausted")
                    .await;
            }
            self.budget -= 1;
            attempts += 1;
            let (observation, _) = self.observe(&operation, &readiness.step.assertions).await;
            let record = vp::judge_step(Phase::Readiness, None, &readiness.step, &operation, observation, attempts, &self.policy);
            if record.verdict == Verdict::Passed || attempts >= readiness.attempts {
                return self.push(record).await;
            }
            self.ports.verification_driver.pause(Duration::from_millis(readiness.interval_ms)).await;
        }
    }

    async fn skip(&mut self, phase: Phase, journey: Option<&str>, step: &Step, verdict: Verdict, reason: &str) -> Verdict {
        let record = vp::not_run(phase, journey, step, verdict, reason, &self.policy);
        self.push(record).await
    }

    /// 保存一個步驟的紀錄，並寫入這次嘗試的日誌（已完成的操作與判定）。
    async fn push(&mut self, record: StepRecord) -> Verdict {
        let verdict = record.verdict;
        self.record(JournalEntry::StepCompleted { record: record.clone() }).await;
        self.steps.push(record);
        verdict
    }

    /// 寫入這次嘗試的日誌；寫不進去（控制資料庫故障，或這次嘗試已不是執行中）時記下問題並回傳 false。
    async fn record(&mut self, entry: JournalEntry) -> bool {
        let now = self.ports.clock.now();
        match self.store.append_verification_journal(self.run_id, self.attempt, entry.clone(), now).await {
            Ok(true) => {
                self.journal.push(entry);
                true
            }
            Ok(false) => {
                self.problem("這次嘗試已不是執行中，驗證日誌沒有寫入".into());
                false
            }
            Err(error) => {
                self.problem(format!("驗證日誌沒有寫入：{error}"));
                false
            }
        }
    }

    fn problem(&mut self, message: String) {
        if !self.problems.contains(&message) {
            self.problems.push(message);
        }
    }

    /// 模型限流（票 49）：另有帳號可以接手時，這個步驟之後交接；沒有時留在原地，之後的語意判斷不呼叫模型。
    async fn hand_off_if_possible(&mut self, limited: String) {
        self.excluded.push(limited);
        match judge_accounts(self.store, &self.excluded).await {
            Ok(accounts) if !accounts.is_empty() => self.handoff = Some(va::REASON_JUDGE_RATE_LIMITED),
            Ok(_) => {}
            Err(error) => self.problem(format!("無法判斷有沒有承接帳號，不交接：{error}")),
        }
    }

    /// 承接（票 49）：以先前嘗試日誌中的參照停止留下的服務與瀏覽器 Session，記下停止了什麼、哪些無法確認。
    async fn take_over(&mut self, takeover: &mut Takeover, previous: &[Attempt]) {
        let left = va::leftovers(previous);
        for handle in left.services {
            match self.ports.verification_driver.stop_service(&ServiceHandle(handle.clone())).await {
                Ok(()) => {
                    self.record(JournalEntry::ServiceStopped { handle: handle.clone() }).await;
                    takeover.stopped_services.push(handle);
                }
                Err(error) => {
                    let message = format!("無法確認服務「{handle}」已停止：{error}");
                    takeover.problems.push(vp::sanitize(&message, &self.policy));
                }
            }
        }
        for session in left.sessions {
            // 未完成旅程的 trace 與錄影不保存：那個案例會重跑，或標為未完成
            match self.ports.browser_driver.close(&BrowserSession(session.clone())).await {
                Ok(_) => {
                    self.record(JournalEntry::BrowserClosed { session: session.clone() }).await;
                    takeover.closed_sessions.push(session);
                }
                Err(error) => {
                    let message = format!("無法確認瀏覽器 Session「{session}」已關閉：{error}");
                    takeover.problems.push(vp::sanitize(&message, &self.policy));
                }
            }
        }
    }

    /// 前置階段的步驟：受阻之後不再執行。
    async fn prerequisite(&mut self, phase: Phase, step: &Step) {
        if self.blocked {
            return;
        }
        if self.run_step(phase, None, step).await != Verdict::Passed {
            self.blocked = true;
        }
    }

    /// 依序執行一組步驟；`stopped` 有值時不執行、以那個原因記錄。回傳之後的步驟應以什麼原因略過（全部通過時不變）。
    async fn run_steps(
        &mut self,
        phase: Phase,
        journey: &str,
        steps: &[Step],
        mut stopped: Option<&'static str>,
        on_failure: &'static str,
    ) -> Option<&'static str> {
        for step in steps {
            // 要交接：目前的旅程停在這裡，由承接的嘗試從新的 fixture 整條重跑
            if self.handoff.is_some() {
                break;
            }
            if let Some(reason) = stopped {
                self.skip(phase, Some(journey), step, Verdict::Inconclusive, reason).await;
            } else if self.run_step(phase, Some(journey), step).await != Verdict::Passed {
                stopped = Some(on_failure);
            }
        }
        stopped
    }

    async fn journey(&mut self, journey: &Journey) {
        let key = journey.key.as_str();
        if self.blocked {
            self.run_steps(Phase::Journey, key, &journey.steps, Some("blocked_by_prerequisite"), "not_executed").await;
            return;
        }
        let definition = self.definition;
        self.persona = journey.persona.as_deref().and_then(|p| definition.persona(p)).map(|p| {
            if p.description.trim().is_empty() { p.title.clone() } else { format!("{}：{}", p.title, p.description) }
        });
        let mut stopped = None;
        let mut opened = true;
        if definition.interface == Interface::Web && !self.open_browser(key).await {
            stopped = Some("blocked_by_prerequisite");
            opened = false;
        }
        // 登入身分：先登入，再核對實際登入的是誰；persona 不參與
        if let Some(identity) = journey.identity.as_deref().and_then(|i| definition.identity(i)) {
            stopped = self.run_steps(Phase::Login, key, &identity.login, stopped, "blocked_by_login").await;
            let check = std::slice::from_ref(&identity.check);
            stopped = self.run_steps(Phase::Identity, key, check, stopped, "blocked_by_identity").await;
        }
        self.run_steps(Phase::Journey, key, &journey.steps, stopped, "not_executed").await;
        self.close_browser().await;
        self.persona = None;
        // 旅程完整結束：最近確認的邊界，承接的嘗試可以沿用（瀏覽器沒有開啟的旅程沒有真的執行，不算）
        if self.handoff.is_none() && opened {
            self.record(JournalEntry::JourneyCompleted { journey: key.to_string() }).await;
        }
    }

    /// 為旅程開啟隔離的瀏覽器 Session；無法開啟時記為環境阻礙並回傳 false。
    async fn open_browser(&mut self, journey: &str) -> bool {
        let request = BrowserSessionRequest {
            working_dir: self.subject_dir.clone(),
            scratch_dir: PathBuf::from(&self.state_dir).join("browser").join(journey),
            trace: self.policy.trace,
            video: self.policy.video,
        };
        match self.ports.browser_driver.open(request).await {
            Ok(session) => {
                self.record(JournalEntry::BrowserOpened { journey: journey.to_string(), session: session.0.clone() }).await;
                let mask = self.policy.mask.clone();
                let journey = journey.to_string();
                let engine = self.ports.browser_driver.engine(&session).unwrap_or_else(|| replay::UNKNOWN_ENGINE.to_string());
                let page = PageView::default();
                self.browser =
                    Some(Browser { session, journey, secret_entered: false, secret_shown: false, mask, engine, page });
                true
            }
            Err(error) => {
                let mut record =
                    system_record(Phase::Start, "browser", "啟動瀏覽器", "browser_unavailable", error.to_string(), &self.policy);
                record.journey = Some(journey.to_string());
                self.push(record).await;
                false
            }
        }
    }

    /// 關閉目前的瀏覽器 Session，依政策保存 trace 與錄影。
    async fn close_browser(&mut self) {
        let Some(browser) = self.browser.take() else { return };
        match self.ports.browser_driver.close(&browser.session).await {
            Ok(files) => {
                self.record(JournalEntry::BrowserClosed { session: browser.session.0.clone() }).await;
                // 關閉後 Session 已不存在，保存時仍需知道它輸入過秘密
                let secret_shown = browser.secret_shown;
                self.browser = Some(Browser { mask: Vec::new(), ..browser });
                for file in files {
                    let kind = if file.kind == "video" { EvidenceArtifactKind::Video } else { EvidenceArtifactKind::Trace };
                    let journey = self.browser.as_ref().map(|b| b.journey.clone());
                    let name = format!("{}-{}", journey.as_deref().unwrap_or("profile"), file.name);
                    self.store_artifact(kind, journey.as_deref(), None, name, file.bytes, secret_shown).await;
                }
                self.browser = None;
            }
            Err(error) => self.problems.push(format!("無法關閉旅程「{}」的瀏覽器 Session：{error}", browser.journey)),
        }
    }

    /// 保存一個證據檔案（內容定址，寫在成果儲存的 `verification\`）；依政策不能保存時只記下原因。
    /// `secret_shown`：截圖時畫面顯示了秘密，或 trace 與錄影的 Session 畫面顯示過秘密。
    async fn store_artifact(
        &mut self,
        kind: EvidenceArtifactKind,
        journey: Option<&str>,
        step: Option<&str>,
        name: String,
        bytes: Vec<u8>,
        secret_shown: bool,
    ) {
        let secret_entered = self.browser.as_ref().is_some_and(|b| b.secret_entered);
        let mut artifact = EvidenceArtifact {
            kind,
            journey: journey.map(str::to_string),
            step: step.map(str::to_string),
            name: vp::sanitize(&name, &self.policy),
            storage_path: None,
            sha256: None,
            size_bytes: None,
            withheld: None,
        };
        if let Some(reason) = vp::withhold_reason(kind, &bytes, &self.policy.redact, secret_entered, secret_shown) {
            artifact.withheld = Some(reason.into());
            return self.keep_artifact(artifact).await;
        }
        let sha256 = sha256_hex(&bytes);
        let extension = match kind {
            EvidenceArtifactKind::Screenshot => "png",
            EvidenceArtifactKind::Trace => "zip",
            EvidenceArtifactKind::Video => "webm",
        };
        let storage_path = format!("{}/{sha256}.{extension}", vp::EVIDENCE_ARTIFACTS_DIR);
        match self.ports.workspace_fs.write_artifact_atomic(&self.artifacts_root.join(&storage_path), &bytes).await {
            Ok(()) => {
                artifact.storage_path = Some(storage_path);
                artifact.sha256 = Some(sha256);
                artifact.size_bytes = Some(bytes.len() as u64);
            }
            Err(error) => {
                artifact.withheld = Some("storage_failed".into());
                self.problems.push(format!("證據檔案「{}」無法保存：{error}", artifact.name));
            }
        }
        self.keep_artifact(artifact).await;
    }

    /// 記下一個證據檔案（含沒有保存的原因），並寫入日誌：沿用旅程時連同它的證據一起沿用。
    async fn keep_artifact(&mut self, artifact: EvidenceArtifact) {
        self.record(JournalEntry::ArtifactStored { artifact: artifact.clone() }).await;
        self.artifacts.push(artifact);
    }

    /// teardown 的一個操作：命令結束碼為 0 或 HTTP 2xx 才算成功；失敗只記在清理問題中，不改變判定。
    async fn teardown(&mut self, index: usize, operation: &Operation) {
        let operation = self.bind(operation);
        let step = Step {
            key: format!("teardown-{}", index + 1),
            title: "teardown".into(),
            assertions: match &operation {
                Operation::Http { .. } => Vec::new(),
                _ => vec![Assertion::ExitCode { equals: 0 }],
            },
            operation: operation.clone(),
            external: false,
        };
        let (observation, _) = if operation.secret_keys().iter().any(|k| self.unresolved.contains(k)) {
            (Observation::NotStarted { reason: "引用的秘密解析不到".into() }, None)
        } else {
            self.observe(&operation, &step.assertions).await
        };
        let succeeded = matches!(&observation, Observation::Responded { status, .. } if (200..300).contains(status));
        let mut record = vp::judge_step(Phase::Teardown, None, &step, &operation, observation, 1, &self.policy);
        if succeeded {
            record.verdict = Verdict::Passed;
            record.reason_code = "teardown_succeeded".into();
        }
        if record.verdict != Verdict::Passed {
            self.problems.push(format!("teardown「{}」沒有成功（{}）", record.operation, record.reason_code));
        }
        self.steps.push(record);
    }
}

/// 不屬於 Profile 步驟的系統紀錄（例如狀態目錄、服務或瀏覽器無法準備）。
fn system_record(phase: Phase, key: &str, title: &str, reason_code: &str, output: String, policy: &EvidencePolicy) -> StepRecord {
    StepRecord {
        phase,
        journey: None,
        key: key.into(),
        title: title.into(),
        operation: String::new(),
        verdict: Verdict::EnvironmentBlocked,
        reason_code: reason_code.into(),
        engine_code: None,
        assertions: Vec::new(),
        output: vp::sanitize(&output, policy),
        attempts: 1,
    }
}

/// 依 Profile 執行驗證要求的一次嘗試（`bundle_id` 由呼叫端產生）：完成時回傳 Evidence Bundle；
/// 語意 judge 限流而另有帳號可以接手時，停止這次的測試程序後回傳交接（票 49）。
pub async fn execute(context: ExecutionContext<'_>, item: &PendingVerificationRun, bundle_id: String) -> AttemptOutcome {
    let ports = context.ports;
    let started_at = context.previous.first().map_or_else(|| ports.clock.now(), |a| a.started_at);
    let definition = &item.profile.definition;
    let request = &item.request;
    let state = state_dir(context.data_dir, &request.id);
    let driver = &ports.verification_driver;

    // 秘密：只解析 Profile 宣告的參照；值加入遮蔽清單，之後所有證據文字與 judge 的輸入都會遮蔽
    let mut policy = definition.evidence.clone();
    let mut secrets = Vec::new();
    let mut unresolved = Vec::new();
    for secret in &definition.secrets {
        match ports.secret_resolver.resolve(&secret.reference).await {
            Ok(value) if !value.is_empty() => {
                policy.redact.push(value.clone());
                secrets.push((secret.key.clone(), value));
            }
            _ => unresolved.push(secret.key.clone()),
        }
    }
    // 較長的值先遮蔽，避免較短的值只遮掉其中一段
    policy.redact.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));

    let no_live_model = request.no_live_model;
    // no_live_model：模型 Port 換成一律拒絕的實作，即使有遺漏的呼叫也不會到達模型
    let model: Arc<dyn SemanticJudge> = if no_live_model { Arc::new(NoLiveModel) } else { ports.semantic_judge.clone() };
    let mut run = Execution {
        ports,
        definition,
        policy,
        subject_dir: PathBuf::from(&item.subject_dir),
        run_id: &request.id,
        state_dir: state.to_string_lossy().into_owned(),
        artifacts_root: context.artifacts_root.to_path_buf(),
        budget: definition.budget.max_operations,
        model_budget: definition.budget.max_model_calls,
        judge_account: context.judge_account,
        judge_rate_limited: false,
        model,
        no_live_model,
        replay: &item.replay,
        actions: Vec::new(),
        usage: VerificationUsage::default(),
        store: context.store,
        attempt: context.attempt.attempt,
        excluded: va::rate_limited_accounts(context.previous),
        handoff: None,
        journal: Vec::new(),
        secrets,
        unresolved,
        browser: None,
        persona: None,
        steps: Vec::new(),
        artifacts: Vec::new(),
        problems: Vec::new(),
        blocked: false,
    };

    // 承接：先以日誌中的安全參照停止前一次留下的測試程序；無法確認停止時不執行任何操作（舊程序可能仍在回應）
    // 舊程序可能仍在使用同一個狀態目錄：受阻時也不準備或移除狀態目錄、不執行 teardown（AI-429）
    let mut takeover_blocked = false;
    let mut takeover = context.previous.last().map(|last| Takeover { from_attempt: last.attempt, ..Takeover::default() });
    if let Some(takeover) = takeover.as_mut() {
        run.take_over(takeover, context.previous).await;
        if !takeover.problems.is_empty() {
            run.blocked = true;
            takeover_blocked = true;
            let output = takeover.problems.join("；");
            let reason = va::REASON_PREVIOUS_SESSION_UNCONFIRMED;
            run.push(system_record(Phase::Start, "takeover", "停止前一次嘗試的測試程序", reason, output, &run.policy)).await;
        }
    }

    let prepared = if takeover_blocked { Ok(()) } else { driver.prepare_state_dir(&state).await };
    if let Err(error) = prepared {
        run.blocked = true;
        let record = system_record(Phase::Start, "state-dir", "準備狀態目錄", "state_dir_unavailable", error.to_string(), &run.policy);
        run.push(record).await;
    }
    let candidate_revision = match driver.revision(&run.subject_dir).await {
        Ok(revision) => revision,
        Err(error) => {
            run.problems.push(format!("無法讀取 candidate revision：{error}"));
            None
        }
    };
    let mut build = candidate_revision.as_ref().map(|r| format!("revision:{r}"));
    if let Some(identity) = &definition.build {
        let step = Step {
            key: "build".into(),
            title: "build 識別".into(),
            operation: Operation::Command { program: identity.program.clone(), args: identity.args.clone(), timeout_ms: None },
            assertions: vec![Assertion::ExitCode { equals: 0 }],
            external: false,
        };
        build = None;
        run.prerequisite(Phase::Build, &step).await;
        if let Some(record) = run.steps.last().filter(|r| r.phase == Phase::Build && r.verdict == Verdict::Passed) {
            build = record.output.lines().map(str::trim).filter(|l| !l.is_empty()).last().map(str::to_string);
        }
    }
    let build = build.map(|b| vp::sanitize(&b, &run.policy));
    run.record(JournalEntry::Build { candidate_revision: candidate_revision.clone(), build: build.clone() }).await;
    for step in &definition.doctor {
        run.prerequisite(Phase::Doctor, step).await;
    }
    let mut service: Option<ServiceHandle> = None;
    if let (Some(start), false) = (&definition.start, run.blocked) {
        let request = ServiceRequest {
            program: vp::substitute(&start.program, run.run_id, &run.state_dir),
            args: start.args.iter().map(|a| vp::substitute(a, run.run_id, &run.state_dir)).collect(),
            working_dir: run.subject_dir.clone(),
        };
        match driver.start_service(request).await {
            Ok(handle) => {
                run.record(JournalEntry::ServiceStarted { handle: handle.0.clone() }).await;
                service = Some(handle);
            }
            Err(error) => {
                run.blocked = true;
                let record = system_record(Phase::Start, "start", "啟動服務", "service_not_started", error.to_string(), &run.policy);
                run.push(record).await;
            }
        }
    }
    if let (Some(readiness), false) = (&definition.readiness, run.blocked) {
        if run.run_readiness(readiness).await != Verdict::Passed {
            run.blocked = true;
        }
    }
    for step in &definition.setup {
        run.prerequisite(Phase::Setup, step).await;
    }

    // 旅程：承接時依先前的日誌沿用已完成的、不重送已開始的外部操作，其餘從新的 fixture 整條執行
    let journeys: Vec<&Journey> = definition.journeys.iter().filter(|j| request.journeys.contains(&j.key)).collect();
    let plans: Vec<JourneyPlan> = journeys
        .iter()
        .map(|j| match context.previous {
            [] => JourneyPlan::Run,
            previous => va::plan_journey(j, previous, candidate_revision.as_deref(), build.as_deref()),
        })
        .collect();
    if let Some(takeover) = takeover.as_mut() {
        for (journey, plan) in journeys.iter().zip(&plans) {
            match plan {
                JourneyPlan::Run => {}
                JourneyPlan::Reuse { .. } => takeover.reused_journeys.push(journey.key.clone()),
                JourneyPlan::NotResent { attempt, step, reason, .. } => takeover.not_resent.push(va::NotResent {
                    journey: journey.key.clone(),
                    step: step.clone(),
                    reason: reason.to_string(),
                    attempt: *attempt,
                }),
            }
        }
        run.record(JournalEntry::TakenOver { takeover: takeover.clone() }).await;
    }
    // (旅程, 結果採用自哪一次嘗試) 與不重跑的旅程
    let mut sources: Vec<(String, u32)> = Vec::new();
    let mut not_resent: Vec<(String, &'static str)> = Vec::new();
    for (journey, plan) in journeys.iter().zip(plans) {
        if run.handoff.is_some() {
            break;
        }
        match plan {
            JourneyPlan::Run => {
                run.journey(journey).await;
                sources.push((journey.key.clone(), run.attempt));
            }
            JourneyPlan::Reuse { attempt, steps, artifacts } => {
                run.steps.extend(steps);
                run.artifacts.extend(artifacts);
                sources.push((journey.key.clone(), attempt));
            }
            JourneyPlan::NotResent { attempt, step, reason, steps, artifacts } => {
                run.steps.extend(va::not_resent_records(journey, &steps, &step, reason, &run.policy));
                run.artifacts.extend(artifacts);
                sources.push((journey.key.clone(), attempt));
                not_resent.push((journey.key.clone(), reason));
            }
        }
    }

    // 清理：一定執行（交接前也是，先停止舊的測試程序），只碰本次驗證的狀態；
    // 承接受阻時除外：狀態目錄可能仍屬於無法確認停止的舊程序
    if !takeover_blocked {
        for (index, operation) in definition.teardown.iter().enumerate() {
            run.teardown(index, operation).await;
        }
    }
    let service_stopped = match &service {
        None => None,
        Some(handle) => match driver.stop_service(handle).await {
            Ok(()) => {
                run.record(JournalEntry::ServiceStopped { handle: handle.0.clone() }).await;
                Some(true)
            }
            Err(error) => {
                run.problems.push(format!("無法停止服務：{error}"));
                Some(false)
            }
        },
    };
    let state_dir_removed = if takeover_blocked {
        run.problems.push("無法確認前一次嘗試的測試程序已停止：沒有執行 teardown，狀態目錄保留不動".into());
        false
    } else {
        match driver.remove_state_dir(&state).await {
            Ok(()) => true,
            Err(error) => {
                run.problems.push(format!("無法移除狀態目錄：{error}"));
                false
            }
        }
    };
    if let Some(reason) = run.handoff {
        return AttemptOutcome::HandedOff { reason };
    }

    let results: Vec<_> = journeys
        .iter()
        .map(|j| match not_resent.iter().find(|(key, _)| *key == j.key) {
            Some((_, reason)) => va::not_resent_result(j, &run.steps, reason),
            None => vp::judge_journey(j, &run.steps),
        })
        .collect();
    let (verdict, reason_code) = vp::aggregate(&run.steps, &results);
    let planned: Vec<String> = journeys.iter().map(|j| j.key.clone()).collect();
    let problems: Vec<String> = run.problems.iter().map(|p| vp::sanitize(p, &run.policy)).collect();
    let finished_at = ports.clock.now();
    let current = Attempt {
        status: AttemptStatus::Completed,
        ended_at: Some(finished_at),
        journal: std::mem::take(&mut run.journal),
        ..context.attempt.clone()
    };
    let attempts: Vec<Attempt> = context.previous.iter().cloned().chain(std::iter::once(current)).collect();
    AttemptOutcome::Completed(Box::new(EvidenceBundle {
        id: bundle_id,
        workspace_id: request.workspace_id.clone(),
        request_id: request.id.clone(),
        task_id: request.task_id.clone(),
        run_id: item.run.as_ref().map(|(id, _)| id.clone()),
        generation: item.run.as_ref().map(|(_, generation)| *generation),
        requirement_version: item.requirement_version,
        profile_version_id: item.profile.id.clone(),
        profile_version: item.profile.version,
        candidate_revision,
        build,
        subject_dir: item.subject_dir.clone(),
        covered_features: vp::covered_features(definition, &planned),
        uncovered: vp::uncovered_by_run(definition, &planned),
        journeys: results,
        steps: run.steps,
        verdict,
        reason_code,
        cleanup: CleanupRecord { state_dir: run.state_dir, state_dir_removed, service_stopped, problems },
        artifacts: run.artifacts,
        no_live_model,
        actions: run.actions,
        usage: run.usage,
        attempts: va::summarize(&attempts, &sources),
        started_at,
        finished_at,
    }))
}
