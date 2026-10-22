//! Verification Adapter（票 45、46，spec D-29）：依 Verification Profile 執行一次驗證並產生 Evidence Bundle。
//!
//! 操作與判定分離：這裡只經由 Port 執行操作（`VerificationDriver` 的命令與 HTTP、`BrowserDriver` 的瀏覽器操作）、
//! 呼叫受限的語意 judge（`SemanticJudge`），把結果轉成 Domain 的 `Observation`，判定一律交給
//! `baton_domain::verification_profile`。順序：
//! 準備本次專屬的狀態目錄 → build 識別 → doctor → 啟動服務 → 就緒（重試）→ 資料準備 → 旅程 → teardown → 停止服務 →
//! 移除狀態目錄。前置階段受阻時旅程不執行；旅程中一步沒有通過，之後的步驟不執行；teardown 與清理一定執行，
//! 清理只碰本次驗證的狀態目錄與它啟動的服務，證據保存在回傳的 Bundle。這裡不寫入控制資料庫。
//!
//! web（票 46）：
//! - 每條旅程一個隔離的瀏覽器 Session；旅程綁定登入身分時，先執行該身分的登入步驟，再以 `check` 核對實際登入的身分，
//!   都通過才執行旅程步驟。persona 只作為 judge 的領域背景，不決定以誰登入。
//! - 秘密：開始時解析 Profile 宣告的參照，值只在交給 Runner 的操作中代入；證據的文字、judge 的輸入都以值遮蔽，
//!   證據檔案含有值或可能記錄了輸入的秘密時不保存。
//! - 語意 judge：輸入只有當前畫面與 Profile 中的固定預期；模型限流之後同一次驗證不再呼叫模型。

use std::path::{Path, PathBuf};
use std::time::Duration;

use baton_domain::artifact::sha256_hex;
use baton_domain::verification_profile::{
    self as vp, Assertion, BrowserAction, CleanupRecord, EvidenceArtifact, EvidenceArtifactKind, EvidenceBundle,
    EvidencePolicy, Interface, Journey, Judgement, Observation, Operation, Phase, ProfileDefinition, ScreenshotPolicy,
    Step, StepRecord, Verdict,
};
use baton_ports::{
    BrowserCommand, BrowserOutcome, BrowserSession, BrowserSessionRequest, BrowserStep, CommandOutput, CommandRequest,
    HttpOutcome, HttpRequest, JudgeAccount, JudgeOutcome, JudgeRequest, PageSnapshot, Ports, ServiceHandle, ServiceRequest,
};
use baton_store::PendingVerificationRun;

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
    /// 語意 judge 使用的帳號；沒有可用帳號時為 `None`（需要 judge 的斷言無法判定）。
    pub judge_account: Option<JudgeAccount>,
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
    /// 剩餘的模型呼叫次數。
    model_budget: u32,
    judge_account: Option<JudgeAccount>,
    /// 這次驗證中模型已經限流：之後的語意判斷不再呼叫模型。
    judge_rate_limited: bool,
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

fn browser_command(action: &BrowserAction) -> BrowserCommand {
    match action.clone() {
        BrowserAction::Goto { url } => BrowserCommand::Goto { url },
        BrowserAction::Click { selector } => BrowserCommand::Click { selector },
        BrowserAction::Fill { selector, text } => BrowserCommand::Fill { selector, text },
        BrowserAction::Press { selector, key } => BrowserCommand::Press { selector, key },
        BrowserAction::WaitFor { selector } => BrowserCommand::WaitFor { selector },
    }
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
                let entered_secret = !operation.secret_keys().is_empty();
                return self.observe_browser(&action, entered_secret, operation.filled_selector(), timeout, assertions).await;
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
        action: &BrowserAction,
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
            command: browser_command(action),
            timeout,
            visible: visible.clone(),
            screenshot_mask: screenshots.then(|| browser.mask.clone()),
        };
        let session = browser.session.clone();
        match self.ports.browser_driver.step(&session, step).await {
            Ok(BrowserOutcome::Done { mut page }) => {
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
                let screenshot = page.and_then(|mut p| self.screenshot(&mut p));
                (Observation::ActionFailed { error, message }, screenshot)
            }
            Ok(BrowserOutcome::Unreachable { reason }) => (Observation::Unreachable { reason }, None),
            Ok(BrowserOutcome::TimedOut) => (Observation::TimedOut { output: String::new() }, None),
            Err(error) => (Observation::ActionFailed { error: "RunnerError".into(), message: error.to_string() }, None),
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
            let judgement = if self.judge_rate_limited {
                Judgement::RateLimited
            } else if self.model_budget == 0 {
                Judgement::BudgetExhausted
            } else if let Some(account) = self.judge_account.clone() {
                self.model_budget -= 1;
                let text: String = redact(&page.text, &self.policy).chars().take(vp::JUDGE_PAGE_TEXT_MAX_CHARS).collect();
                let request = JudgeRequest {
                    account,
                    expectation: expectation.clone(),
                    persona: self.persona.clone(),
                    page_url: redact(&page.url, &self.policy),
                    page_title: redact(&page.title, &self.policy),
                    page_text: text,
                };
                match self.ports.semantic_judge.judge(request).await {
                    Ok(JudgeOutcome::Decided { satisfied, rationale }) => Judgement::Decided { satisfied, rationale },
                    Ok(JudgeOutcome::RateLimited { .. }) => {
                        self.judge_rate_limited = true;
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
            return self.skip(phase, journey, step, Verdict::EnvironmentBlocked, "secret_unavailable");
        }
        let automated = !matches!(operation, Operation::Manual { .. });
        if automated && self.budget == 0 {
            return self.skip(phase, journey, step, Verdict::BudgetExhausted, "operation_budget_exhausted");
        }
        if automated {
            self.budget -= 1;
        }
        let (observation, screenshot) = self.observe(&operation, &step.assertions).await;
        let record = vp::judge_step(phase, journey, step, &operation, observation, 1, &self.policy);
        if let Some(Screenshot { bytes, shows_secret }) = screenshot {
            if self.policy.screenshots == ScreenshotPolicy::EveryStep || record.verdict != Verdict::Passed {
                let name = format!("{}-{}.png", journey.unwrap_or("profile"), step.key);
                self.store_artifact(EvidenceArtifactKind::Screenshot, journey, Some(&step.key), name, bytes, shows_secret).await;
            }
        }
        self.push(record)
    }

    /// 就緒條件：最多嘗試 `attempts` 次，兩次之間等待 `interval_ms`；只保存最後一次的觀察與總次數。
    async fn run_readiness(&mut self, readiness: &vp::Readiness) -> Verdict {
        let operation = self.bind(&readiness.step.operation);
        let mut attempts = 0;
        loop {
            if self.budget == 0 {
                return self.skip(Phase::Readiness, None, &readiness.step, Verdict::BudgetExhausted, "operation_budget_exhausted");
            }
            self.budget -= 1;
            attempts += 1;
            let (observation, _) = self.observe(&operation, &readiness.step.assertions).await;
            let record = vp::judge_step(Phase::Readiness, None, &readiness.step, &operation, observation, attempts, &self.policy);
            if record.verdict == Verdict::Passed || attempts >= readiness.attempts {
                return self.push(record);
            }
            self.ports.verification_driver.pause(Duration::from_millis(readiness.interval_ms)).await;
        }
    }

    fn skip(&mut self, phase: Phase, journey: Option<&str>, step: &Step, verdict: Verdict, reason: &str) -> Verdict {
        let record = vp::not_run(phase, journey, step, verdict, reason, &self.policy);
        self.push(record)
    }

    fn push(&mut self, record: StepRecord) -> Verdict {
        let verdict = record.verdict;
        self.steps.push(record);
        verdict
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
            if let Some(reason) = stopped {
                self.skip(phase, Some(journey), step, Verdict::Inconclusive, reason);
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
        if definition.interface == Interface::Web && !self.open_browser(key).await {
            stopped = Some("blocked_by_prerequisite");
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
                let mask = self.policy.mask.clone();
                let journey = journey.to_string();
                self.browser = Some(Browser { session, journey, secret_entered: false, secret_shown: false, mask });
                true
            }
            Err(error) => {
                let mut record =
                    system_record(Phase::Start, "browser", "啟動瀏覽器", "browser_unavailable", error.to_string(), &self.policy);
                record.journey = Some(journey.to_string());
                self.steps.push(record);
                false
            }
        }
    }

    /// 關閉目前的瀏覽器 Session，依政策保存 trace 與錄影。
    async fn close_browser(&mut self) {
        let Some(browser) = self.browser.take() else { return };
        match self.ports.browser_driver.close(&browser.session).await {
            Ok(files) => {
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
            self.artifacts.push(artifact);
            return;
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

/// 依 Profile 執行一個驗證要求，回傳 Evidence Bundle（`bundle_id` 由呼叫端產生）。
pub async fn execute(context: ExecutionContext<'_>, item: &PendingVerificationRun, bundle_id: String) -> EvidenceBundle {
    let ports = context.ports;
    let started_at = ports.clock.now();
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
        secrets,
        unresolved,
        browser: None,
        persona: None,
        steps: Vec::new(),
        artifacts: Vec::new(),
        problems: Vec::new(),
        blocked: false,
    };

    if let Err(error) = driver.prepare_state_dir(&state).await {
        run.blocked = true;
        let record = system_record(Phase::Start, "state-dir", "準備狀態目錄", "state_dir_unavailable", error.to_string(), &run.policy);
        run.steps.push(record);
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
            Ok(handle) => service = Some(handle),
            Err(error) => {
                run.blocked = true;
                let record = system_record(Phase::Start, "start", "啟動服務", "service_not_started", error.to_string(), &run.policy);
                run.steps.push(record);
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
    let journeys: Vec<&Journey> = definition.journeys.iter().filter(|j| request.journeys.contains(&j.key)).collect();
    for journey in &journeys {
        run.journey(journey).await;
    }

    // 清理：一定執行，只碰本次驗證的狀態
    for (index, operation) in definition.teardown.iter().enumerate() {
        run.teardown(index, operation).await;
    }
    let service_stopped = match &service {
        None => None,
        Some(handle) => match driver.stop_service(handle).await {
            Ok(()) => Some(true),
            Err(error) => {
                run.problems.push(format!("無法停止服務：{error}"));
                Some(false)
            }
        },
    };
    let state_dir_removed = match driver.remove_state_dir(&state).await {
        Ok(()) => true,
        Err(error) => {
            run.problems.push(format!("無法移除狀態目錄：{error}"));
            false
        }
    };

    let results: Vec<_> = journeys.iter().map(|j| vp::judge_journey(j, &run.steps)).collect();
    let (verdict, reason_code) = vp::aggregate(&run.steps, &results);
    let planned: Vec<String> = journeys.iter().map(|j| j.key.clone()).collect();
    let problems: Vec<String> = run.problems.iter().map(|p| vp::sanitize(p, &run.policy)).collect();
    EvidenceBundle {
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
        build: build.map(|b| vp::sanitize(&b, &run.policy)),
        subject_dir: item.subject_dir.clone(),
        covered_features: vp::covered_features(definition, &planned),
        uncovered: vp::uncovered_by_run(definition, &planned),
        journeys: results,
        steps: run.steps,
        verdict,
        reason_code,
        cleanup: CleanupRecord { state_dir: run.state_dir, state_dir_removed, service_stopped, problems },
        artifacts: run.artifacts,
        attempts: Vec::new(),
        started_at,
        finished_at: ports.clock.now(),
    }
}
