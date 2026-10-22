//! Gate 失效與重驗（票 50，issue #3 §9.2、spec 使用者故事 23、73）。
//!
//! 已通過的 Gate 不是永久有效：Runtime 重新查驗它的依賴，依賴改變時 Gate 失效並建立重驗工作。
//! - **依賴就是證據**：每個通過的 Stage 保存了它的證據（成果、裁定、成果接受、審查、Evidence Bundle）與通過當下的 build。
//!   重新查驗時證據不再有效（規格版本改變使審查過期、Profile 或需求版本改變使 Bundle 不算數…）就失效。
//! - **build**：只有審查與驗證證據依賴 build；通過時記下的 build 與受驗目錄目前的 build 不同就失效。
//!   build 觀察不到（不是 Git repo）時不比較。成果、裁定與成果接受不依賴 build，不受影響。
//! - **假設被推翻**：Task 的契約依據的裁定被使用者以不同於建議的答案裁定時，除了使用者的裁定本身，其他 Gate 都失效。
//! - **只影響依賴改變的 Gate**：其他 Gate 保持通過。Workflow 回到最早失效的 Stage，記下原本的目前 Stage（`resume_stage`）；
//!   重驗通過後，這之間仍有效通過的 Stage 直接沿用，不重跑。

use super::*;

/// Runtime 重新查驗一個 Workflow 已通過 Gate 所需的事實。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GateFacts {
    /// 每個目前有效通過的 Stage（[`passed_attempts`]）重新查驗後的證據：(Stage key, 證據)。
    pub rechecked: Vec<(String, Vec<CheckedEvidence>)>,
    /// 受驗目錄目前的 build（revision）；觀察不到時為 `None`，不比較 build。
    pub current_build: Option<String>,
    /// Workflow 的 Task 依據的假設已被使用者推翻的說明。
    pub basis_overturned: Option<String>,
}

/// Gate 失效的結果：新的 Workflow 狀態、每個失效 Stage 一筆 `invalidated` attempt，以及一個事件。
#[derive(Debug, Clone, PartialEq)]
pub struct GateInvalidation {
    pub workflow: WorkflowInstance,
    pub attempts: Vec<StageAttempt>,
    pub event: DomainEvent,
}

/// 目前有效通過（不是跳過、也還沒失效）的 Stage 各自最後一次通過的 attempt，依 Playbook 順序。
/// 包含目前 Stage 之前的，以及 Gate 失效後重驗途中、`resume_stage` 之前仍保留的通過。
pub fn passed_attempts<'a>(
    workflow: &WorkflowInstance,
    playbook: &PlaybookVersion,
    attempts: &'a [StageAttempt],
) -> Vec<&'a StageAttempt> {
    let states = stage_states(workflow, playbook, attempts);
    playbook
        .definition
        .stages
        .iter()
        .zip(states)
        .filter(|(_, state)| *state == StageState::Passed)
        .filter_map(|(stage, _)| {
            attempts
                .iter()
                .filter(|a| a.stage_key == stage.key)
                .filter(|a| matches!(a.outcome, AttemptOutcome::Passed | AttemptOutcome::Skipped | AttemptOutcome::Invalidated))
                .max_by_key(|a| a.attempt)
                .filter(|a| a.outcome == AttemptOutcome::Passed)
        })
        .collect()
}

/// 這個 Stage 的證據是否依賴 build（審查與驗證）。
fn depends_on_build(stage: &StageDefinition) -> bool {
    stage.exit_checks.iter().any(|c| matches!(c.requires, EvidenceKind::Review | EvidenceKind::Verification))
}

/// 一個已通過的 Stage 失效的原因；仍有效時是空的。
fn causes(stage: &StageDefinition, passed: &StageAttempt, facts: &GateFacts) -> Vec<MissingCheck> {
    let mut causes = Vec::new();
    if let Some(why) = &facts.basis_overturned {
        // 使用者的裁定本身不依賴假設；其他證據都是在被推翻的前提下產生的
        for check in stage.exit_checks.iter().filter(|c| c.requires != EvidenceKind::Decision) {
            causes.push(MissingCheck { check: check.key.clone(), reason: format!("Task 依據的假設已被推翻：{why}") });
        }
        if !causes.is_empty() {
            return causes;
        }
    }
    if let (Some(then), Some(now)) = (&passed.build, &facts.current_build) {
        if then != now && depends_on_build(stage) {
            for check in stage.exit_checks.iter().filter(|c| matches!(c.requires, EvidenceKind::Review | EvidenceKind::Verification)) {
                causes.push(MissingCheck {
                    check: check.key.clone(),
                    reason: format!("build 已改變：通過時是 {then}，目前是 {now}"),
                });
            }
            return causes;
        }
    }
    if let Some((_, evidence)) = facts.rechecked.iter().find(|(key, _)| *key == stage.key) {
        let (outcome, missing, _) = evaluate(stage, evidence);
        if outcome != AttemptOutcome::Passed {
            causes = missing;
        }
    }
    causes
}

/// 重新查驗已通過的 Gate：有 Gate 失效時回傳變更，全部仍有效時回 `None`。完成的 Workflow 不重新查驗。
pub fn revalidate(
    workflow: &WorkflowInstance,
    playbook: &PlaybookVersion,
    attempts: &[StageAttempt],
    facts: &GateFacts,
    now: Timestamp,
) -> Option<GateInvalidation> {
    let current = playbook.definition.stage_index(workflow.current_stage.as_deref()?)?;
    let mut next = workflow.clone();
    next.revision += 1;
    next.updated_at = now;
    let mut invalidated = Vec::new();
    for passed in passed_attempts(workflow, playbook, attempts) {
        let Some(stage) = playbook.definition.stage(&passed.stage_key) else { continue };
        let causes = causes(stage, passed, facts);
        if causes.is_empty() {
            continue;
        }
        let summary: Vec<&str> = causes.iter().map(|c| c.reason.as_str()).collect();
        invalidated.push(StageAttempt {
            id: format!("{}_inv{}_{}", workflow.id, next.revision, stage.key),
            workflow_id: workflow.id.clone(),
            stage_key: stage.key.clone(),
            attempt: next_attempt_no(attempts, &stage.key),
            outcome: AttemptOutcome::Invalidated,
            evidence: Vec::new(),
            note: format!("Gate 失效，需要重驗：{}", summary.join("；")),
            missing: causes,
            recorded_by: RECORDED_BY_RUNTIME.to_string(),
            profile_id: None,
            build: facts.current_build.clone(),
            created_at: now,
        });
    }
    let earliest = invalidated.iter().filter_map(|a| playbook.definition.stage_index(&a.stage_key)).min()?;
    if earliest < current {
        // 記下最遠的進度：再次失效時不會把它往回改
        let resume = next.resume_stage.as_deref().and_then(|r| playbook.definition.stage_index(r)).unwrap_or(current).max(current);
        next.resume_stage = Some(playbook.definition.stages[resume].key.clone());
        next.current_stage = Some(playbook.definition.stages[earliest].key.clone());
        next.status = WorkflowStatus::Active;
        next.blocked_reason = None;
    }
    let event = workflow_event(
        &next,
        EVENT_GATE_INVALIDATED,
        serde_json::json!({
            "workspace_id": next.workspace_id,
            "task_id": next.task_id,
            "stages": invalidated.iter().map(|a| serde_json::json!({
                "stage": a.stage_key,
                "attempt_id": a.id,
                "causes": a.missing,
            })).collect::<Vec<_>>(),
            "current_stage": next.current_stage,
            "resume_stage": next.resume_stage,
            "status": next.status.as_str(),
        }),
    );
    Some(GateInvalidation { workflow: next, attempts: invalidated, event })
}
