//! 內建的方法與七個 Playbook（issue #3 §8.1、§9.1）。
//!
//! 內容改變時 Runtime 啟動會追加新版本（不覆寫舊版本）；進行中的 Workflow 仍使用它開始時的版本。

use super::{
    EvidenceKind, ExitCheck, FailureRoute, GateKind, MethodDefinition, PlaybookDefinition, Situation, StageBudget,
    StageDefinition,
};

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[allow(clippy::too_many_arguments)]
fn method(
    key: &str,
    title: &str,
    purpose: &str,
    inputs: &[&str],
    outputs: &[&str],
    applicability: &str,
    tool_requirements: &[&str],
    budget: &str,
    stop_conditions: &[&str],
    verification: &str,
    source: &str,
) -> MethodDefinition {
    MethodDefinition {
        key: key.into(),
        title: title.into(),
        purpose: purpose.into(),
        inputs: strings(inputs),
        outputs: strings(outputs),
        applicability: applicability.into(),
        tool_requirements: strings(tool_requirements),
        budget: budget.into(),
        stop_conditions: strings(stop_conditions),
        verification: verification.into(),
        source: source.into(),
    }
}

const MATTPOCOCK: &str = "mattpocock skills（issue #3 §8.1）";
const PSTACK: &str = "pstack claude（issue #3 §8.1）";
const TESTER_ARMY: &str = "tester army e2e（issue #3 §8.1）";

/// 內建的 Method Registry。
pub fn builtin_methods() -> Vec<MethodDefinition> {
    vec![
        method(
            "design_interview",
            "設計問答與尖銳問題",
            "以具體情境追問，分清已確認事項、假設與真正需要使用者取捨的問題",
            &["目前的 Project Brief", "使用者的目標描述"],
            &["問題清單（可觀測事實／可實驗假說／產品偏好／權限裁定）", "情境範例"],
            "需求仍模糊、成功條件不清楚時",
            &["讀取 repo 與文件"],
            "一輪問答；只問會阻擋下一步的關鍵問題",
            &["關鍵問題都已分類", "需要取捨的問題已送裁定"],
            "每個問題都標明分類與依據",
            MATTPOCOCK,
        ),
        method(
            "domain_glossary",
            "領域詞彙",
            "把專案的名詞、定義與關係落成 Glossary，與執行進度分開",
            &["裁定結果", "既有 Glossary"],
            &["新增或修正的 Glossary 項目"],
            "需求釐清後、名詞有歧義時",
            &["record_understanding"],
            "一次更新",
            &["裁定中出現的名詞都有定義"],
            "每個定義附來源",
            MATTPOCOCK,
        ),
        method(
            "design_alternatives",
            "替代設計比較",
            "比較至少兩個結構不同的設計，說明取捨與失敗模式",
            &["需求契約", "Project Map"],
            &["Design Package（領域模型、狀態轉換、介面契約、失敗模式、測試觀察點）"],
            "非平凡的架構變更、高影響或有爭議的設計",
            &["讀取 repo"],
            "兩個草案或一個最小實驗",
            &["選定設計並記錄理由"],
            "設計說明可以對照需求契約",
            PSTACK,
        ),
        method(
            "vertical_slice",
            "垂直切片",
            "把工作切成可展示或驗證的垂直切片，標明依賴與並行條件",
            &["設計"],
            &["切片清單與依賴"],
            "功能開發、可驗證單位的拆分",
            &["propose_task", "add_dependency"],
            "一次拆分",
            &["每個切片都有可驗收的完成條件"],
            "切片能各自通過驗收",
            MATTPOCOCK,
        ),
        method(
            "behavior_tests",
            "行為測試",
            "先寫描述外部行為的測試，再實作到通過",
            &["切片的完成條件"],
            &["測試與實作", "測試結果"],
            "會修改程式的工作",
            &["編輯程式", "執行測試"],
            "依 Stage 預算",
            &["新增的測試與既有測試都通過"],
            "測試只斷言外部行為",
            MATTPOCOCK,
        ),
        method(
            "real_interface_verification",
            "真實介面驗證",
            "以真實介面（CLI、API、網頁）操作並斷言，行動與判定分離",
            &["可執行的成果", "驗收條件"],
            &["驗證證據（操作紀錄、斷言結果）"],
            "成果有使用者可操作的介面時",
            &["執行應用程式", "瀏覽器或 HTTP 客戶端"],
            "依 Stage 預算",
            &["每項驗收條件都有對應的證據"],
            "證據可以重現",
            TESTER_ARMY,
        ),
        method(
            "two_axis_review",
            "雙軸審查",
            "由非作者分開審查 Standards 與 Spec 兩軸，作者摘要不是驗收證據",
            &["base／candidate revision", "需求契約"],
            &["審查報告與 finding"],
            "整合前",
            &["讀取差異"],
            "一次審查；修正後只重審受影響部分",
            &["兩軸都有結論"],
            "reviewer 不是作者",
            MATTPOCOCK,
        ),
        method(
            "reproduction",
            "重現與最小化",
            "建立能捕捉原症狀的重現，再縮到最小",
            &["缺陷描述或疑點"],
            &["重現步驟或測試", "最小化的案例"],
            "缺陷處理、探索測試的疑點確認",
            &["執行應用程式", "執行測試"],
            "依 Stage 預算",
            &["重現穩定", "已排除環境因素"],
            "重現可以由別人執行",
            TESTER_ARMY,
        ),
        method(
            "falsifiable_hypothesis",
            "可否證假說與探測",
            "提出可以被證據推翻的假說，設計探測並記錄結果",
            &["重現或基線"],
            &["假說與探測結果"],
            "缺陷原因、效能變化不明時",
            &["執行程式", "量測工具"],
            "依 Stage 預算",
            &["假說被證實或推翻"],
            "探測結果可重跑",
            PSTACK,
        ),
        method(
            "baseline_measurement",
            "基線與同條件量測",
            "定義指標與代表負載，量測基線，在相同條件下比較",
            &["指標定義", "代表負載"],
            &["量測結果與可重跑的量測腳本"],
            "效能改善",
            &["量測工具"],
            "依 Stage 預算",
            &["前後量測條件相同"],
            "量測可以重跑",
            PSTACK,
        ),
        method(
            "primary_source_research",
            "一手來源研究",
            "以一手來源或實驗回答明確的問題，標明證據與不確定性",
            &["明確的問題"],
            &["研究報告（結論、證據、不確定性）"],
            "調查與研究、需求釐清中的研究",
            &["讀取文件", "網路研究"],
            "依 Stage 預算",
            &["問題已回答，或確認需要裁定"],
            "每個結論附來源",
            PSTACK,
        ),
        method(
            "exploratory_charter",
            "探索章程",
            "劃分探索範圍、角色與預算，在隔離環境中收集疑點",
            &["要探索的功能"],
            &["探索章程", "疑點清單"],
            "探索測試",
            &["執行應用程式"],
            "依章程的時間與次數",
            &["章程範圍都探索過"],
            "疑點附操作紀錄",
            TESTER_ARMY,
        ),
    ]
}

/// 一個 exit check。
fn check(key: &str, description: &str, requires: EvidenceKind) -> ExitCheck {
    ExitCheck { key: key.into(), description: description.into(), requires, reviewers: None }
}

fn artifact(key: &str, description: &str) -> ExitCheck {
    check(key, description, EvidenceKind::Artifact)
}

struct Stage {
    key: &'static str,
    title: &'static str,
    method: Option<&'static str>,
    entry: &'static [&'static str],
    outputs: &'static [&'static str],
    actions: &'static [&'static str],
    max_attempts: u32,
    checks: Vec<ExitCheck>,
    failure: FailureRoute,
    gate: GateKind,
}

impl Stage {
    fn build(self) -> StageDefinition {
        StageDefinition {
            key: self.key.into(),
            title: self.title.into(),
            method: self.method.map(str::to_string),
            entry_conditions: strings(self.entry),
            expected_outputs: strings(self.outputs),
            allowed_actions: strings(self.actions),
            budget: StageBudget { max_attempts: self.max_attempts },
            exit_checks: self.checks,
            failure_route: self.failure,
            gate: self.gate,
        }
    }
}

fn back_to(stage: &str) -> FailureRoute {
    FailureRoute::ReturnTo { stage: stage.into() }
}

fn playbook(key: &str, title: &str, situation: Situation, applicability: &str, stages: Vec<Stage>) -> PlaybookDefinition {
    PlaybookDefinition {
        key: key.into(),
        title: title.into(),
        situation,
        applicability: applicability.into(),
        stages: stages.into_iter().map(Stage::build).collect(),
    }
}

const READ: &str = "讀取 repo 與文件";
const RESEARCH: &str = "研究與實驗（不修改正式程式）";
const EDIT: &str = "修改程式";
const TEST: &str = "執行測試";
const RUN_APP: &str = "執行應用程式";
const ASK: &str = "送出裁定請求";
const RECORD: &str = "登記成果";

/// 內建的七個 Playbook（issue §9.1 的表格）。
pub fn builtin_playbooks() -> Vec<PlaybookDefinition> {
    use EvidenceKind::{Decision, Review, TaskAccepted};
    use FailureRoute::{Block, Retry};
    use GateKind::{Decision as DecisionGate, None as Plain, Safety};
    vec![
        playbook(
            "ambiguous",
            "需求釐清",
            Situation::Ambiguous,
            "需求仍模糊、成功條件或範圍不清楚；不進行正式實作",
            vec![
                Stage { key: "read_current_state", title: "閱讀現況", method: None, entry: &["Workspace 已接入"], outputs: &["現況摘要（已確認、假設、缺口）"], actions: &[READ, RECORD], max_attempts: 3, checks: vec![artifact("current_state", "現況摘要")], failure: Retry, gate: Plain },
                Stage { key: "sharp_questions", title: "尖銳問題與情境", method: Some("design_interview"), entry: &["現況摘要已登記"], outputs: &["分類過的問題清單與情境"], actions: &[READ, RECORD], max_attempts: 3, checks: vec![artifact("questions", "分類過的問題清單")], failure: Retry, gate: Plain },
                Stage { key: "research_prototype", title: "研究／原型", method: Some("primary_source_research"), entry: &["可觀測或可實驗的問題已列出"], outputs: &["研究或原型的結果"], actions: &[READ, RESEARCH, RECORD], max_attempts: 3, checks: vec![artifact("findings", "研究或原型的結果")], failure: Retry, gate: Plain },
                Stage { key: "decision", title: "裁定", method: None, entry: &["產品取捨已附證據"], outputs: &["使用者的裁定"], actions: &[ASK], max_attempts: 3, checks: vec![check("decision", "使用者已回答的裁定", Decision)], failure: Block, gate: DecisionGate },
                Stage { key: "update_brief", title: "更新 Brief 與詞彙", method: Some("domain_glossary"), entry: &["裁定已回答"], outputs: &["更新後的 Project Brief 與 Glossary"], actions: &[READ, "record_understanding", RECORD], max_attempts: 3, checks: vec![artifact("brief_update", "Brief 與詞彙的更新摘要")], failure: Retry, gate: Plain },
            ],
        ),
        playbook(
            "feature",
            "功能開發",
            Situation::Feature,
            "新增或改變使用者可見的功能，會修改程式",
            vec![
                Stage { key: "requirements_contract", title: "需求契約", method: Some("design_interview"), entry: &["目標已明確"], outputs: &["需求契約與驗收條件"], actions: &[READ, ASK, RECORD], max_attempts: 3, checks: vec![artifact("contract", "需求契約與驗收條件")], failure: Retry, gate: Plain },
                Stage { key: "design", title: "設計", method: Some("design_alternatives"), entry: &["需求契約已登記"], outputs: &["Design Package 或沿用既有模式的說明"], actions: &[READ, RESEARCH, RECORD], max_attempts: 3, checks: vec![artifact("design", "設計說明")], failure: Retry, gate: Plain },
                Stage { key: "slicing", title: "垂直切片與依賴", method: Some("vertical_slice"), entry: &["設計已登記"], outputs: &["切片清單與依賴"], actions: &[READ, "propose_task", RECORD], max_attempts: 3, checks: vec![artifact("slices", "切片清單與依賴")], failure: Retry, gate: Plain },
                Stage { key: "tests_and_implementation", title: "行為測試及實作", method: Some("behavior_tests"), entry: &["切片已定義"], outputs: &["測試、實作與測試結果"], actions: &[READ, EDIT, TEST, RECORD], max_attempts: 3, checks: vec![artifact("test_results", "測試與實作的結果")], failure: Retry, gate: Plain },
                Stage { key: "real_interface_verification", title: "真實介面驗證", method: Some("real_interface_verification"), entry: &["測試通過"], outputs: &["真實介面的驗證證據"], actions: &[READ, RUN_APP, TEST, RECORD], max_attempts: 3, checks: vec![artifact("verification", "真實介面的驗證證據")], failure: back_to("tests_and_implementation"), gate: Safety },
                Stage { key: "independent_review", title: "獨立審查", method: Some("two_axis_review"), entry: &["真實介面驗證通過"], outputs: &["非作者的雙軸審查報告"], actions: &[READ, RECORD], max_attempts: 3, checks: vec![check("review", "非作者 reviewer 的雙軸審查（Standards 與 Spec）都通過", Review)], failure: back_to("tests_and_implementation"), gate: Safety },
                Stage { key: "integration", title: "整合", method: None, entry: &["審查通過"], outputs: &["通過成果接受的 Task"], actions: &[READ, TEST], max_attempts: 3, checks: vec![check("accepted", "Task 已通過成果接受", TaskAccepted)], failure: back_to("tests_and_implementation"), gate: Safety },
            ],
        ),
        playbook(
            "defect",
            "缺陷處理",
            Situation::Defect,
            "已知或疑似的缺陷，修復需要修改程式",
            vec![
                Stage { key: "reproduction", title: "建立重現", method: Some("reproduction"), entry: &["缺陷描述"], outputs: &["能捕捉原症狀的重現"], actions: &[READ, RUN_APP, TEST, RECORD], max_attempts: 3, checks: vec![artifact("reproduction", "能捕捉原症狀的重現")], failure: Retry, gate: Plain },
                Stage { key: "minimization", title: "最小化", method: Some("reproduction"), entry: &["重現已登記"], outputs: &["最小化的案例"], actions: &[READ, RUN_APP, TEST, RECORD], max_attempts: 3, checks: vec![artifact("minimal_case", "最小化的案例")], failure: Retry, gate: Plain },
                Stage { key: "hypothesis", title: "可否證假說", method: Some("falsifiable_hypothesis"), entry: &["最小案例已登記"], outputs: &["可被證據推翻的假說"], actions: &[READ, RECORD], max_attempts: 3, checks: vec![artifact("hypothesis", "假說與預期的探測結果")], failure: Retry, gate: Plain },
                Stage { key: "probe", title: "探測", method: Some("falsifiable_hypothesis"), entry: &["假說已登記"], outputs: &["探測結果（證實或推翻假說）"], actions: &[READ, RUN_APP, TEST, RECORD], max_attempts: 3, checks: vec![artifact("probe_results", "探測結果")], failure: back_to("hypothesis"), gate: Plain },
                Stage { key: "fix", title: "修復", method: Some("behavior_tests"), entry: &["原因已證實"], outputs: &["修復與對應的測試"], actions: &[READ, EDIT, TEST, RECORD], max_attempts: 3, checks: vec![artifact("fix", "修復與測試")], failure: back_to("hypothesis"), gate: Plain },
                Stage { key: "regression_verification", title: "原案例與回歸驗證", method: Some("real_interface_verification"), entry: &["修復已登記"], outputs: &["原案例不再發生、回歸測試通過的證據"], actions: &[READ, RUN_APP, TEST, RECORD], max_attempts: 3, checks: vec![artifact("original_case", "原案例不再發生的證據"), artifact("regression", "回歸測試結果")], failure: back_to("fix"), gate: Safety },
            ],
        ),
        playbook(
            "migration",
            "架構或廣泛遷移",
            Situation::Migration,
            "跨模組的架構變更或機械性遷移，採 expand、migrate、contract",
            vec![
                Stage { key: "current_state", title: "現況及原理", method: None, entry: &["遷移目標已明確"], outputs: &["現況與設計原理的說明"], actions: &[READ, RECORD], max_attempts: 3, checks: vec![artifact("current_state", "現況與原理")], failure: Retry, gate: Plain },
                Stage { key: "alternatives", title: "替代設計", method: Some("design_alternatives"), entry: &["現況已登記"], outputs: &["至少兩個替代設計與選擇理由"], actions: &[READ, RESEARCH, RECORD], max_attempts: 3, checks: vec![artifact("alternatives", "替代設計比較")], failure: Retry, gate: Plain },
                Stage { key: "impact", title: "影響範圍", method: None, entry: &["設計已選定"], outputs: &["受影響的模組、契約與資料"], actions: &[READ, RECORD], max_attempts: 3, checks: vec![artifact("impact", "影響範圍")], failure: Retry, gate: Plain },
                Stage { key: "verifiable_units", title: "可驗證單位", method: Some("vertical_slice"), entry: &["影響範圍已登記"], outputs: &["可各自驗證的遷移單位與相依"], actions: &[READ, "propose_task", RECORD], max_attempts: 3, checks: vec![artifact("units", "遷移單位與相依")], failure: Retry, gate: Plain },
                Stage { key: "migrate", title: "遷移", method: Some("behavior_tests"), entry: &["遷移單位已定義"], outputs: &["遷移後的程式與測試結果"], actions: &[READ, EDIT, TEST, RECORD], max_attempts: 3, checks: vec![artifact("migration_results", "遷移與測試結果")], failure: Retry, gate: Plain },
                Stage { key: "contract_check", title: "契約核對", method: None, entry: &["遷移完成"], outputs: &["新舊契約一致的核對結果"], actions: &[READ, TEST, RECORD], max_attempts: 3, checks: vec![artifact("contract_check", "契約核對結果")], failure: back_to("migrate"), gate: Safety },
                Stage { key: "remove_old_path", title: "刪除舊路徑", method: None, entry: &["契約核對通過"], outputs: &["舊路徑已移除、測試通過"], actions: &[READ, EDIT, TEST, RECORD], max_attempts: 3, checks: vec![artifact("removal", "移除舊路徑後的測試結果")], failure: back_to("migrate"), gate: Plain },
            ],
        ),
        playbook(
            "performance",
            "效能改善",
            Situation::Performance,
            "改善可量測的效能指標",
            vec![
                Stage { key: "metrics_and_load", title: "定義指標及代表負載", method: Some("baseline_measurement"), entry: &["效能目標已明確"], outputs: &["指標與代表負載的定義"], actions: &[READ, RECORD], max_attempts: 3, checks: vec![artifact("metrics", "指標與代表負載")], failure: Retry, gate: Plain },
                Stage { key: "baseline", title: "基線", method: Some("baseline_measurement"), entry: &["指標已定義"], outputs: &["基線量測結果"], actions: &[READ, RUN_APP, RECORD], max_attempts: 3, checks: vec![artifact("baseline", "基線量測")], failure: Retry, gate: Plain },
                Stage { key: "hypothesis_experiment", title: "假說與實驗", method: Some("falsifiable_hypothesis"), entry: &["基線已登記"], outputs: &["假說與實驗變更"], actions: &[READ, EDIT, RUN_APP, RECORD], max_attempts: 3, checks: vec![artifact("experiment", "假說與實驗結果")], failure: Retry, gate: Plain },
                Stage { key: "comparison", title: "同條件比較", method: Some("baseline_measurement"), entry: &["實驗完成"], outputs: &["與基線同條件的比較"], actions: &[READ, RUN_APP, RECORD], max_attempts: 3, checks: vec![artifact("comparison", "同條件比較")], failure: back_to("hypothesis_experiment"), gate: Safety },
                Stage { key: "explanation", title: "解釋變化", method: None, entry: &["比較完成"], outputs: &["變化的解釋"], actions: &[READ, RECORD], max_attempts: 3, checks: vec![artifact("explanation", "變化的解釋")], failure: Retry, gate: Plain },
                Stage { key: "repeatable_measurement", title: "保留可重跑量測", method: Some("baseline_measurement"), entry: &["變化已解釋"], outputs: &["可重跑的量測腳本"], actions: &[READ, EDIT, RECORD], max_attempts: 3, checks: vec![artifact("measurement_script", "可重跑的量測")], failure: Retry, gate: Plain },
            ],
        ),
        playbook(
            "investigation",
            "調查與研究",
            Situation::Investigation,
            "回答明確的問題；研究與文件工作適用，不自動轉成實作",
            vec![
                Stage { key: "question", title: "明確問題", method: Some("design_interview"), entry: &["有待回答的問題"], outputs: &["明確、可回答的問題"], actions: &[READ, RECORD], max_attempts: 3, checks: vec![artifact("question", "明確的問題")], failure: Retry, gate: Plain },
                Stage { key: "sources_experiments", title: "一手來源／實驗", method: Some("primary_source_research"), entry: &["問題已明確"], outputs: &["一手來源或實驗結果"], actions: &[READ, RESEARCH, RECORD], max_attempts: 3, checks: vec![artifact("sources", "一手來源或實驗結果")], failure: Retry, gate: Plain },
                Stage { key: "evidence_uncertainty", title: "證據與不確定性", method: Some("primary_source_research"), entry: &["來源已收集"], outputs: &["證據整理與不確定性"], actions: &[READ, RECORD], max_attempts: 3, checks: vec![artifact("evidence", "證據與不確定性")], failure: Retry, gate: Plain },
                Stage { key: "conclusion", title: "結論或裁定", method: None, entry: &["證據已整理"], outputs: &["結論，或附證據送出的裁定"], actions: &[READ, ASK, RECORD], max_attempts: 3, checks: vec![artifact("conclusion", "結論（或裁定請求與證據）")], failure: Retry, gate: Plain },
            ],
        ),
        playbook(
            "exploratory_testing",
            "探索測試",
            Situation::ExploratoryTesting,
            "以探索找出未知缺陷，確認後修復並回歸",
            vec![
                Stage { key: "charter", title: "劃分探索章程與角色", method: Some("exploratory_charter"), entry: &["要探索的功能已指定"], outputs: &["探索章程與角色"], actions: &[READ, RECORD], max_attempts: 3, checks: vec![artifact("charter", "探索章程")], failure: Retry, gate: Plain },
                Stage { key: "isolated_execution", title: "隔離執行", method: Some("exploratory_charter"), entry: &["章程已登記"], outputs: &["隔離環境中的探索紀錄"], actions: &[READ, RUN_APP, RECORD], max_attempts: 3, checks: vec![artifact("session_log", "探索紀錄")], failure: Retry, gate: Plain },
                Stage { key: "collect_suspicions", title: "收集疑點", method: None, entry: &["探索完成"], outputs: &["疑點清單"], actions: &[READ, RECORD], max_attempts: 3, checks: vec![artifact("suspicions", "疑點清單")], failure: Retry, gate: Plain },
                Stage { key: "triage", title: "排除環境與預期行為", method: None, entry: &["疑點已列出"], outputs: &["排除後仍待確認的疑點"], actions: &[READ, RUN_APP, RECORD], max_attempts: 3, checks: vec![artifact("triage", "排除結果")], failure: Retry, gate: Plain },
                Stage { key: "reproduction", title: "重現確認", method: Some("reproduction"), entry: &["待確認的疑點"], outputs: &["已確認缺陷的重現"], actions: &[READ, RUN_APP, TEST, RECORD], max_attempts: 3, checks: vec![artifact("reproduction", "已確認缺陷的重現")], failure: back_to("triage"), gate: Safety },
                Stage { key: "fix_and_regression", title: "修復及回歸", method: Some("behavior_tests"), entry: &["缺陷已確認"], outputs: &["修復與回歸驗證"], actions: &[READ, EDIT, TEST, RUN_APP, RECORD], max_attempts: 3, checks: vec![artifact("fix", "修復"), artifact("regression", "回歸驗證")], failure: Retry, gate: Safety },
            ],
        ),
    ]
}
