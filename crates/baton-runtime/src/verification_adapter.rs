//! CLI／API Verification Adapter（票 45，spec D-29）：依 Verification Profile 執行一次驗證並產生 Evidence Bundle。
//!
//! 操作與判定分離：這裡只經由 `VerificationDriver` Port 執行操作、把結果轉成 Domain 的 `Observation`，
//! 判定一律交給 `baton_domain::verification_profile`。順序：
//! 準備本次專屬的狀態目錄 → build 識別 → doctor → 啟動服務 → 就緒（重試）→ 資料準備 → 旅程 → teardown → 停止服務 →
//! 移除狀態目錄。前置階段受阻時旅程不執行；旅程中一步沒有通過，之後的步驟不執行；teardown 與清理一定執行，
//! 清理只碰本次驗證的狀態目錄與它啟動的服務，證據保存在回傳的 Bundle。這裡不寫入控制資料庫。

use std::path::{Path, PathBuf};
use std::time::Duration;

use baton_domain::verification_profile::{
    self as vp, Assertion, CleanupRecord, EvidenceBundle, EvidencePolicy, Journey, Observation, Operation, Phase, Step,
    StepRecord, Verdict,
};
use baton_ports::{CommandOutput, CommandRequest, HttpOutcome, HttpRequest, Ports, ServiceHandle, ServiceRequest};
use baton_store::PendingVerificationRun;

/// 本次驗證專屬的狀態目錄：`<資料目錄>\verification\<驗證要求>`。
pub fn state_dir(data_dir: &Path, request_id: &str) -> PathBuf {
    data_dir.join("verification").join(request_id)
}

/// 執行中的一次驗證。
struct Execution<'a> {
    ports: &'a Ports,
    policy: &'a EvidencePolicy,
    subject_dir: PathBuf,
    run_id: &'a str,
    state_dir: String,
    /// 剩餘的操作次數（不含 teardown）。
    budget: u32,
    steps: Vec<StepRecord>,
    /// 前置階段已經受阻：之後不再執行任何操作（teardown 除外）。
    blocked: bool,
}

impl Execution<'_> {
    async fn observe(&self, operation: &Operation) -> Observation {
        let timeout = Duration::from_millis(operation.effective_timeout_ms());
        let driver = &self.ports.verification_driver;
        match operation {
            Operation::Command { program, args, .. } => {
                let request =
                    CommandRequest { program: program.clone(), args: args.clone(), working_dir: self.subject_dir.clone(), timeout };
                match driver.run_command(request).await {
                    Ok(CommandOutput::Exited { code, output }) => Observation::Exited { code, output },
                    Ok(CommandOutput::Terminated { output }) => Observation::Terminated { output },
                    Ok(CommandOutput::TimedOut { output }) => Observation::TimedOut { output },
                    Err(error) => Observation::NotStarted { reason: error.to_string() },
                }
            }
            Operation::Http { method, url, headers, body, .. } => {
                let request = HttpRequest {
                    method: method.clone(),
                    url: url.clone(),
                    headers: headers.iter().map(|h| (h.name.clone(), h.value.clone())).collect(),
                    body: body.clone(),
                    timeout,
                };
                match driver.send_http(request).await {
                    Ok(HttpOutcome::Responded { status, body }) => Observation::Responded { status, body },
                    Ok(HttpOutcome::Unreachable { reason }) => Observation::Unreachable { reason },
                    Ok(HttpOutcome::TimedOut) => Observation::TimedOut { output: String::new() },
                    Err(error) => Observation::Unreachable { reason: error.to_string() },
                }
            }
            Operation::Browser { .. } => Observation::Unsupported { reason: "尚未實作瀏覽器操作".into() },
            Operation::Manual { instruction } => Observation::Unsupported {
                reason: format!("CLI／API Adapter 無法自動執行人工步驟：{instruction}"),
            },
        }
    }

    fn bind(&self, operation: &Operation) -> Operation {
        vp::bind_operation(operation, self.run_id, &self.state_dir)
    }

    /// 執行並判定一個步驟；需要操作而預算已經用盡時判定為 `budget_exhausted`。人工步驟不消耗預算。
    async fn run_step(&mut self, phase: Phase, journey: Option<&str>, step: &Step) -> Verdict {
        let operation = self.bind(&step.operation);
        let automated = !matches!(operation, Operation::Manual { .. });
        if automated && self.budget == 0 {
            return self.skip(phase, journey, step, Verdict::BudgetExhausted, "operation_budget_exhausted");
        }
        if automated {
            self.budget -= 1;
        }
        let observation = self.observe(&operation).await;
        let record = vp::judge_step(phase, journey, step, &operation, observation, 1, self.policy);
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
            let observation = self.observe(&operation).await;
            let record = vp::judge_step(Phase::Readiness, None, &readiness.step, &operation, observation, attempts, self.policy);
            if record.verdict == Verdict::Passed || attempts >= readiness.attempts {
                return self.push(record);
            }
            self.ports.verification_driver.pause(Duration::from_millis(readiness.interval_ms)).await;
        }
    }

    fn skip(&mut self, phase: Phase, journey: Option<&str>, step: &Step, verdict: Verdict, reason: &str) -> Verdict {
        self.push(vp::not_run(phase, journey, step, verdict, reason, self.policy))
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

    async fn journey(&mut self, journey: &Journey) {
        let mut stopped = false;
        for step in &journey.steps {
            if self.blocked {
                self.skip(Phase::Journey, Some(&journey.key), step, Verdict::Inconclusive, "blocked_by_prerequisite");
            } else if stopped {
                self.skip(Phase::Journey, Some(&journey.key), step, Verdict::Inconclusive, "not_executed");
            } else if self.run_step(Phase::Journey, Some(&journey.key), step).await != Verdict::Passed {
                stopped = true;
            }
        }
    }

    /// teardown 的一個操作：命令結束碼為 0 或 HTTP 2xx 才算成功；失敗只記在清理問題中，不改變判定。
    async fn teardown(&mut self, index: usize, operation: &Operation, problems: &mut Vec<String>) {
        let operation = self.bind(operation);
        let step = Step {
            key: format!("teardown-{}", index + 1),
            title: "teardown".into(),
            assertions: match &operation {
                Operation::Http { .. } => Vec::new(),
                _ => vec![Assertion::ExitCode { equals: 0 }],
            },
            operation: operation.clone(),
        };
        let observation = self.observe(&operation).await;
        let succeeded = matches!(&observation, Observation::Responded { status, .. } if (200..300).contains(status));
        let mut record = vp::judge_step(Phase::Teardown, None, &step, &operation, observation, 1, self.policy);
        if succeeded {
            record.verdict = Verdict::Passed;
            record.reason_code = "teardown_succeeded".into();
        }
        if record.verdict != Verdict::Passed {
            problems.push(format!("teardown「{}」沒有成功（{}）", record.operation, record.reason_code));
        }
        self.steps.push(record);
    }
}

/// 不屬於 Profile 步驟的系統紀錄（例如狀態目錄或服務無法準備）。
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
pub async fn execute(ports: &Ports, data_dir: &Path, item: &PendingVerificationRun, bundle_id: String) -> EvidenceBundle {
    let started_at = ports.clock.now();
    let definition = &item.profile.definition;
    let policy = &definition.evidence;
    let request = &item.request;
    let state = state_dir(data_dir, &request.id);
    let driver = &ports.verification_driver;
    let mut run = Execution {
        ports,
        policy,
        subject_dir: PathBuf::from(&item.subject_dir),
        run_id: &request.id,
        state_dir: state.to_string_lossy().into_owned(),
        budget: definition.budget.max_operations,
        steps: Vec::new(),
        blocked: false,
    };
    let mut problems = Vec::new();

    if let Err(error) = driver.prepare_state_dir(&state).await {
        run.blocked = true;
        run.steps.push(system_record(Phase::Start, "state-dir", "準備狀態目錄", "state_dir_unavailable", error.to_string(), policy));
    }
    let candidate_revision = match driver.revision(&run.subject_dir).await {
        Ok(revision) => revision,
        Err(error) => {
            problems.push(format!("無法讀取 candidate revision：{error}"));
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
                run.steps.push(system_record(Phase::Start, "start", "啟動服務", "service_not_started", error.to_string(), policy));
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
        run.teardown(index, operation, &mut problems).await;
    }
    let service_stopped = match &service {
        None => None,
        Some(handle) => match driver.stop_service(handle).await {
            Ok(()) => Some(true),
            Err(error) => {
                problems.push(format!("無法停止服務：{error}"));
                Some(false)
            }
        },
    };
    let state_dir_removed = match driver.remove_state_dir(&state).await {
        Ok(()) => true,
        Err(error) => {
            problems.push(format!("無法移除狀態目錄：{error}"));
            false
        }
    };

    let results: Vec<_> = journeys.iter().map(|j| vp::judge_journey(j, &run.steps)).collect();
    let (verdict, reason_code) = vp::aggregate(&run.steps, &results);
    let planned: Vec<String> = journeys.iter().map(|j| j.key.clone()).collect();
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
        build: build.map(|b| vp::sanitize(&b, policy)),
        subject_dir: item.subject_dir.clone(),
        covered_features: vp::covered_features(definition, &planned),
        uncovered: vp::uncovered_by_run(definition, &planned),
        journeys: results,
        steps: run.steps,
        verdict,
        reason_code,
        cleanup: CleanupRecord { state_dir: run.state_dir, state_dir_removed, service_stopped, problems },
        artifacts: Vec::new(),
        started_at,
        finished_at: ports.clock.now(),
    }
}
