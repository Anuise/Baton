//! 決策權限表（票 35，spec D-15）：哪些事項 Controller 可以自主處理、哪些一定要送裁定。
//!
//! 純邏輯、無 I/O。第一版內建一張表；使用者可以把 Workspace 中任何一個事項設為三種等級中的任一種，
//! 收窄或放寬都可以（使用者決定，見 ADR-0003 修訂）。Runtime 對 Controller 的每個動作都以
//! `effective` 判定：必須送裁定的動作不執行，改成建立裁定請求。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 決策權限表中的一個事項。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Matter {
    // ---- 內建可自主處理
    /// 在既有授權內拆分、合併、排序、重試 Task。
    ArrangeTasks,
    /// 選擇 Playbook 與方法版本。
    ChoosePlaybook,
    /// 派研究或實驗 Worker。
    DispatchResearch,
    /// 把 review finding 分類為「修正」。
    ClassifyFindingAsFix,
    /// 在預算內重派。
    RedispatchWithinBudget,
    /// 建立調查 Task。
    CreateInvestigationTask,
    /// 把候選教訓提交給使用者。
    SubmitLessonCandidate,
    // ---- 內建必須送裁定
    /// 任何產品方向、使用者價值或範圍取捨。
    ProductDirection,
    /// 修改完成條件或驗收標準。
    ModifyDoneCriteria,
    /// 擴大或超出 Workspace 授權的操作。
    ExceedAuthorization,
    /// 外部寫入、部署、發訊、付費、刪除正式資料。
    ExternalWrite,
    /// 駁回 review finding。
    DismissReviewFinding,
    /// 跳過 Stage 或 Gate。
    SkipStageOrGate,
    /// 結果未知外部操作的處置。
    UnknownOutcomeOperation,
    /// 接受設計偏離。
    AcceptDesignDeviation,
    /// 預算用盡後的下一步。
    NextStepAfterBudget,
    // ---- 只能人工發起
    /// 三次恢復失敗後的重試（只能由使用者從恢復介面發起）。
    RetryAfterRecoveryFailures,
}

/// 一個事項的權限等級；宣告順序由寬到嚴。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Authority {
    /// Controller 可以自主處理。
    Autonomous,
    /// 必須送裁定：Controller 嘗試時被拒絕並轉成裁定請求。
    RequiresDecision,
    /// 只能由使用者從對應介面發起；Controller 嘗試時被拒絕，也不建立裁定請求。
    HumanOnly,
}

impl Matter {
    pub const ALL: [Matter; 17] = [
        Matter::ArrangeTasks,
        Matter::ChoosePlaybook,
        Matter::DispatchResearch,
        Matter::ClassifyFindingAsFix,
        Matter::RedispatchWithinBudget,
        Matter::CreateInvestigationTask,
        Matter::SubmitLessonCandidate,
        Matter::ProductDirection,
        Matter::ModifyDoneCriteria,
        Matter::ExceedAuthorization,
        Matter::ExternalWrite,
        Matter::DismissReviewFinding,
        Matter::SkipStageOrGate,
        Matter::UnknownOutcomeOperation,
        Matter::AcceptDesignDeviation,
        Matter::NextStepAfterBudget,
        Matter::RetryAfterRecoveryFailures,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Matter::ArrangeTasks => "arrange_tasks",
            Matter::ChoosePlaybook => "choose_playbook",
            Matter::DispatchResearch => "dispatch_research",
            Matter::ClassifyFindingAsFix => "classify_finding_as_fix",
            Matter::RedispatchWithinBudget => "redispatch_within_budget",
            Matter::CreateInvestigationTask => "create_investigation_task",
            Matter::SubmitLessonCandidate => "submit_lesson_candidate",
            Matter::ProductDirection => "product_direction",
            Matter::ModifyDoneCriteria => "modify_done_criteria",
            Matter::ExceedAuthorization => "exceed_authorization",
            Matter::ExternalWrite => "external_write",
            Matter::DismissReviewFinding => "dismiss_review_finding",
            Matter::SkipStageOrGate => "skip_stage_or_gate",
            Matter::UnknownOutcomeOperation => "unknown_outcome_operation",
            Matter::AcceptDesignDeviation => "accept_design_deviation",
            Matter::NextStepAfterBudget => "next_step_after_budget",
            Matter::RetryAfterRecoveryFailures => "retry_after_recovery_failures",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == raw)
    }

    /// 給使用者看的名稱。
    pub fn label(self) -> &'static str {
        match self {
            Matter::ArrangeTasks => "在既有授權內拆分、合併、排序、重試 Task",
            Matter::ChoosePlaybook => "選擇 Playbook 與方法版本",
            Matter::DispatchResearch => "派研究或實驗 Worker",
            Matter::ClassifyFindingAsFix => "把 review finding 分類為「修正」",
            Matter::RedispatchWithinBudget => "在預算內重派",
            Matter::CreateInvestigationTask => "建立調查 Task",
            Matter::SubmitLessonCandidate => "把候選教訓提交給使用者",
            Matter::ProductDirection => "產品方向、使用者價值或範圍取捨",
            Matter::ModifyDoneCriteria => "修改完成條件或驗收標準",
            Matter::ExceedAuthorization => "擴大或超出 Workspace 授權的操作",
            Matter::ExternalWrite => "外部寫入、部署、發訊、付費、刪除正式資料",
            Matter::DismissReviewFinding => "駁回 review finding",
            Matter::SkipStageOrGate => "跳過 Stage 或 Gate",
            Matter::UnknownOutcomeOperation => "結果未知外部操作的處置",
            Matter::AcceptDesignDeviation => "接受設計偏離",
            Matter::NextStepAfterBudget => "預算用盡後的下一步",
            Matter::RetryAfterRecoveryFailures => "三次恢復失敗後的重試（只能從恢復介面發起）",
        }
    }

    /// 內建（第一版）的權限等級。
    pub fn builtin(self) -> Authority {
        match self {
            Matter::ArrangeTasks
            | Matter::ChoosePlaybook
            | Matter::DispatchResearch
            | Matter::ClassifyFindingAsFix
            | Matter::RedispatchWithinBudget
            | Matter::CreateInvestigationTask
            | Matter::SubmitLessonCandidate => Authority::Autonomous,
            Matter::RetryAfterRecoveryFailures => Authority::HumanOnly,
            _ => Authority::RequiresDecision,
        }
    }
}

impl Authority {
    pub fn as_str(self) -> &'static str {
        match self {
            Authority::Autonomous => "autonomous",
            Authority::RequiresDecision => "requires_decision",
            Authority::HumanOnly => "human_only",
        }
    }
}

/// Workspace 的權限表設定：使用者設定、且與內建等級不同的事項。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityPolicy {
    /// 每次修改加一；從未修改為 0。
    pub revision: i64,
    /// 事項 → 使用者設定的等級；只存與內建不同的事項。
    pub overrides: BTreeMap<Matter, Authority>,
}

impl AuthorityPolicy {
    /// 這個 Workspace 中某事項目前生效的等級：有使用者設定就用設定，否則用內建等級。
    pub fn effective(&self, matter: Matter) -> Authority {
        self.overrides.get(&matter).copied().unwrap_or_else(|| matter.builtin())
    }
}

/// 使用者要求的一個事項設定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityEntryRequest {
    pub matter: Matter,
    pub effective: Authority,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthorityError {
    #[error("權限表已被修改（目前 revision {current}），請重新整理後再存檔")]
    RevisionMismatch { current: i64 },
}

impl AuthorityError {
    pub fn code(&self) -> &'static str {
        match self {
            AuthorityError::RevisionMismatch { .. } => "authority_revision_mismatch",
        }
    }
}

/// 以使用者送來的設定取代 Workspace 的權限表（沒列出的事項回到內建值）。任何事項都可以設為任何等級；
/// 同一事項列出多次時以最後一個為準。
pub fn replace_policy(
    current: &AuthorityPolicy,
    expected_revision: i64,
    entries: &[AuthorityEntryRequest],
) -> Result<AuthorityPolicy, AuthorityError> {
    if expected_revision != current.revision {
        return Err(AuthorityError::RevisionMismatch { current: current.revision });
    }
    let mut overrides = BTreeMap::new();
    for entry in entries {
        if entry.effective == entry.matter.builtin() {
            overrides.remove(&entry.matter);
        } else {
            overrides.insert(entry.matter, entry.effective);
        }
    }
    Ok(AuthorityPolicy { revision: current.revision + 1, overrides })
}

/// 權限表的一列（給 API 與設定畫面）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityEntry {
    pub matter: Matter,
    pub label: String,
    pub builtin: Authority,
    pub effective: Authority,
    /// 這個 Workspace 的設定是否與內建不同（收窄或放寬）。
    pub overridden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityTable {
    pub revision: i64,
    pub entries: Vec<AuthorityEntry>,
}

pub fn table(policy: &AuthorityPolicy) -> AuthorityTable {
    AuthorityTable {
        revision: policy.revision,
        entries: Matter::ALL
            .into_iter()
            .map(|matter| {
                let effective = policy.effective(matter);
                AuthorityEntry {
                    matter,
                    label: matter.label().to_string(),
                    builtin: matter.builtin(),
                    effective,
                    overridden: effective != matter.builtin(),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(matter: Matter, effective: Authority) -> AuthorityEntryRequest {
        AuthorityEntryRequest { matter, effective }
    }

    #[test]
    fn the_builtin_table_matches_spec_d15() {
        let autonomous: Vec<Matter> =
            Matter::ALL.into_iter().filter(|m| m.builtin() == Authority::Autonomous).collect();
        assert_eq!(autonomous.len(), 7);
        assert_eq!(Matter::ModifyDoneCriteria.builtin(), Authority::RequiresDecision);
        assert_eq!(Matter::SkipStageOrGate.builtin(), Authority::RequiresDecision);
        assert_eq!(Matter::ExternalWrite.builtin(), Authority::RequiresDecision);
        assert_eq!(Matter::RetryAfterRecoveryFailures.builtin(), Authority::HumanOnly);
        for matter in Matter::ALL {
            assert_eq!(Matter::parse(matter.as_str()), Some(matter));
        }
    }

    #[test]
    fn without_settings_every_matter_is_its_builtin_level() {
        let policy = AuthorityPolicy::default();
        for matter in Matter::ALL {
            assert_eq!(policy.effective(matter), matter.builtin(), "{matter:?}");
        }
    }

    #[test]
    fn a_workspace_can_narrow_an_autonomous_matter() {
        let policy = replace_policy(&AuthorityPolicy::default(), 0, &[entry(Matter::ArrangeTasks, Authority::RequiresDecision)])
            .unwrap();
        assert_eq!(policy.revision, 1);
        assert_eq!(policy.effective(Matter::ArrangeTasks), Authority::RequiresDecision);
        assert_eq!(policy.effective(Matter::ChoosePlaybook), Authority::Autonomous);
        assert!(table(&policy).entries.iter().any(|e| e.matter == Matter::ArrangeTasks && e.overridden));

        // 取代式：沒列出就回到內建值
        let reset = replace_policy(&policy, 1, &[]).unwrap();
        assert_eq!(reset.effective(Matter::ArrangeTasks), Authority::Autonomous);
    }

    #[test]
    fn a_workspace_can_widen_builtin_requires_decision_and_human_only_matters() {
        let policy = replace_policy(
            &AuthorityPolicy::default(),
            0,
            &[
                entry(Matter::ModifyDoneCriteria, Authority::Autonomous),
                entry(Matter::RetryAfterRecoveryFailures, Authority::RequiresDecision),
            ],
        )
        .unwrap();
        assert_eq!(policy.effective(Matter::ModifyDoneCriteria), Authority::Autonomous);
        assert_eq!(policy.effective(Matter::RetryAfterRecoveryFailures), Authority::RequiresDecision);
        assert_eq!(policy.effective(Matter::ExternalWrite), Authority::RequiresDecision, "沒列出的維持內建");

        let policy = replace_policy(&policy, 1, &[entry(Matter::RetryAfterRecoveryFailures, Authority::Autonomous)]).unwrap();
        assert_eq!(policy.effective(Matter::RetryAfterRecoveryFailures), Authority::Autonomous);
        assert_eq!(policy.effective(Matter::ModifyDoneCriteria), Authority::RequiresDecision, "取代式：回到內建值");
    }

    #[test]
    fn every_matter_can_be_set_to_every_level() {
        let levels = [Authority::Autonomous, Authority::RequiresDecision, Authority::HumanOnly];
        for matter in Matter::ALL {
            for level in levels {
                let policy = replace_policy(&AuthorityPolicy::default(), 0, &[entry(matter, level)]).unwrap();
                assert_eq!(policy.effective(matter), level, "{matter:?} → {level:?}");
                let row = table(&policy).entries.into_iter().find(|e| e.matter == matter).unwrap();
                assert_eq!((row.effective, row.overridden), (level, level != matter.builtin()));
            }
        }
    }

    #[test]
    fn only_levels_that_differ_from_builtin_are_stored() {
        let policy = replace_policy(
            &AuthorityPolicy::default(),
            0,
            &[entry(Matter::SkipStageOrGate, Authority::RequiresDecision), entry(Matter::ArrangeTasks, Authority::HumanOnly)],
        )
        .unwrap();
        assert_eq!(policy.overrides, BTreeMap::from([(Matter::ArrangeTasks, Authority::HumanOnly)]));
    }

    #[test]
    fn a_stale_revision_is_refused() {
        let current = AuthorityPolicy { revision: 2, overrides: BTreeMap::new() };
        let refused = replace_policy(&current, 1, &[]).unwrap_err();
        assert_eq!(refused, AuthorityError::RevisionMismatch { current: 2 });
        assert_eq!(refused.code(), "authority_revision_mismatch");
    }
}
