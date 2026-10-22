//! Workspace 授權：以「能力 + 目標資源 + 限制」表達的範圍授權、可編輯範本、確認與 revision，以及 Run 的准入檢查。
//!
//! 純邏輯、無 I/O。選取目錄不等於授權（DR-20261021-03 的 I2）：Workspace 建立後沒有任何授權，
//! 使用者確認並保存後才產生 revision 1，之後每次修改都產生新的 revision 並保存來源與時間。
//! 未確認前，任何需要某能力的 Run 都不能被准入。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::workspace::{Workspace, WorkspaceStatus};
use crate::Timestamp;

pub const AGGREGATE_TYPE: &str = "workspace_authorization";
pub const EVENT_CONFIRMED: &str = "authorization_confirmed";

/// 範本識別：研究／文件。
pub const TEMPLATE_RESEARCH_DOCS: &str = "research_docs";
/// 範本識別：可信專案開發（P3）。
pub const TEMPLATE_TRUSTED_DEV: &str = "trusted_dev_p3";

/// 確認摘要必須揭露的風險（可信程式不受 OS 沙箱隔離）。
pub const TRUSTED_EXECUTION_RISK: &str = "專案與第三方依賴程式會以目前 Windows 使用者的身分執行。Baton 沒有作業系統沙箱，\
工作目錄與 worktree 不是安全邊界，無法攔截這些程式內部的全部副作用。";

/// 桌面管理憑證是目前唯一能確認授權的身分（Agent 憑證不能擴權，spec D-22）。
pub const CONFIRMED_BY_DESKTOP: &str = "desktop";

/// Agent 能使用的能力。宣告順序同時是確認內容的標準排列順序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// 讀取 Workspace 內的檔案。
    ReadFiles,
    /// 編輯 Workspace 內的檔案（工作副本或輸出目錄）。
    WriteFiles,
    /// 讀取公開網頁（唯讀）。
    WebRead,
    /// 在指定環境建置與測試專案（P3）。
    RunBuildTest,
    /// 從指定來源安裝專案依賴（P3）。
    InstallProjectDependencies,
    /// 以系統管理員身分安裝（不在 P3）。
    AdminInstall,
    /// 全域安裝（不在 P3）。
    GlobalInstall,
    /// 遠端寫入，例如 push（不在 P3）。
    RemoteWrite,
    /// 部署（不在 P3）。
    Deploy,
    /// 對外發訊（不在 P3）。
    ExternalMessage,
    /// 付費（不在 P3）。
    Payment,
    /// 正式資料操作（不在 P3）。
    ProductionData,
}

impl Capability {
    pub const ALL: [Capability; 12] = [
        Capability::ReadFiles,
        Capability::WriteFiles,
        Capability::WebRead,
        Capability::RunBuildTest,
        Capability::InstallProjectDependencies,
        Capability::AdminInstall,
        Capability::GlobalInstall,
        Capability::RemoteWrite,
        Capability::Deploy,
        Capability::ExternalMessage,
        Capability::Payment,
        Capability::ProductionData,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Capability::ReadFiles => "read_files",
            Capability::WriteFiles => "write_files",
            Capability::WebRead => "web_read",
            Capability::RunBuildTest => "run_build_test",
            Capability::InstallProjectDependencies => "install_project_dependencies",
            Capability::AdminInstall => "admin_install",
            Capability::GlobalInstall => "global_install",
            Capability::RemoteWrite => "remote_write",
            Capability::Deploy => "deploy",
            Capability::ExternalMessage => "external_message",
            Capability::Payment => "payment",
            Capability::ProductionData => "production_data",
        }
    }

    /// 給使用者看的名稱。
    pub fn label(self) -> &'static str {
        match self {
            Capability::ReadFiles => "讀取檔案",
            Capability::WriteFiles => "編輯檔案",
            Capability::WebRead => "讀取公開網頁",
            Capability::RunBuildTest => "建置與測試",
            Capability::InstallProjectDependencies => "安裝專案依賴",
            Capability::AdminInstall => "管理員安裝",
            Capability::GlobalInstall => "全域安裝",
            Capability::RemoteWrite => "遠端寫入",
            Capability::Deploy => "部署",
            Capability::ExternalMessage => "對外發訊",
            Capability::Payment => "付費",
            Capability::ProductionData => "正式資料操作",
        }
    }

    /// 管理員／全域安裝、遠端寫入、部署、對外發訊、付費與正式資料操作：任何範本都不含，必須各自明確授權。
    pub fn is_high_risk(self) -> bool {
        matches!(
            self,
            Capability::AdminInstall
                | Capability::GlobalInstall
                | Capability::RemoteWrite
                | Capability::Deploy
                | Capability::ExternalMessage
                | Capability::Payment
                | Capability::ProductionData
        )
    }

    /// 會讓專案或第三方依賴程式以使用者的 Windows 身分執行；確認時必須先承認風險。
    pub fn runs_trusted_code(self) -> bool {
        matches!(
            self,
            Capability::RunBuildTest
                | Capability::InstallProjectDependencies
                | Capability::AdminInstall
                | Capability::GlobalInstall
        )
    }
}

/// 一項授權：能力 + 目標資源 + 限制條件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub capability: Capability,
    /// 能力作用的資源（目錄、來源等）；至少一項。
    pub targets: Vec<String>,
    #[serde(default)]
    pub restrictions: Vec<String>,
}

/// 一個 revision 的來源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// 原封不動採用範本。
    Template,
    /// 以範本為起點、使用者編輯過。
    TemplateEdited,
    /// 沒有套用範本，使用者自行組成。
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionSource {
    pub kind: SourceKind,
    pub template_id: Option<String>,
    /// 這次修改是以哪個 revision 為基礎；第一次確認為 `None`。
    pub base_revision: Option<i64>,
    /// 確認者的身分；目前只有桌面管理憑證（`CONFIRMED_BY_DESKTOP`）。
    pub confirmed_by: String,
}

/// 授權的一個不可變 revision；Workspace 目前生效的授權是最新的一個。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationRevision {
    pub workspace_id: String,
    /// 從 1 起連續遞增；與事件的 `aggregate_version` 對應。
    pub revision: i64,
    pub grants: Vec<Grant>,
    pub source: RevisionSource,
    pub confirmed_at: Timestamp,
}

impl AuthorizationRevision {
    pub fn grants_capability(&self, capability: Capability) -> bool {
        self.grants.iter().any(|g| g.capability == capability)
    }
}

/// 建議範本：使用者可以編輯後才確認，沒有任何範本會被自動套用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub description: String,
    pub grants: Vec<Grant>,
}

fn grant(capability: Capability, targets: &[&str], restrictions: &[&str]) -> Grant {
    Grant {
        capability,
        targets: targets.iter().map(|s| s.to_string()).collect(),
        restrictions: restrictions.iter().map(|s| s.to_string()).collect(),
    }
}

/// 全部建議範本，目標資源已代入 Workspace 的目錄。
pub fn templates(workspace_path: &str) -> Vec<Template> {
    let here = [workspace_path];
    let read = grant(Capability::ReadFiles, &here, &[]);
    let write = grant(Capability::WriteFiles, &here, &["僅限 Workspace 目錄內", "不修改 Git 歷史"]);
    vec![
        Template {
            id: TEMPLATE_RESEARCH_DOCS.into(),
            name: "研究／文件".into(),
            description: "讀取與整理資料、撰寫文件與輸出成果；不執行專案程式、不安裝任何東西。".into(),
            grants: vec![
                read.clone(),
                write.clone(),
                grant(Capability::WebRead, &["公開網頁"], &["僅讀取，不登入、不提交表單"]),
            ],
        },
        Template {
            id: TEMPLATE_TRUSTED_DEV.into(),
            name: "可信專案開發（P3）".into(),
            description: "編輯工作副本、在指定環境建置與測試、從指定來源安裝專案依賴；\
                          管理員／全域安裝、遠端寫入、部署、對外發訊、付費與正式資料操作不在其中。"
                .into(),
            grants: vec![
                read,
                write,
                grant(
                    Capability::RunBuildTest,
                    &here,
                    &["僅執行專案內建置與測試指令", "不使用系統管理員身分"],
                ),
                grant(
                    Capability::InstallProjectDependencies,
                    &["專案設定檔與 lockfile 宣告的套件來源"],
                    &["僅安裝到專案目錄內", "不做全域安裝", "不使用系統管理員身分"],
                ),
            ],
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthorizationError {
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("Workspace {0} 已經封存，不能再修改授權")]
    WorkspaceArchived(String),
    #[error("沒有名為 {0} 的授權範本")]
    UnknownTemplate(String),
    #[error("能力「{}」重複出現，同一個能力只能有一項授權", .0.label())]
    DuplicateCapability(Capability),
    #[error("能力「{}」必須指定至少一個目標資源", .0.label())]
    MissingTarget(Capability),
    #[error("授權包含會執行專案或第三方依賴程式、或屬於高風險的能力，必須先確認已了解：{TRUSTED_EXECUTION_RISK}")]
    RiskNotAcknowledged,
}

impl AuthorizationError {
    /// 穩定的機器可讀代碼，API 與 UI 依它判斷錯誤種類。
    pub fn code(&self) -> &'static str {
        match self {
            AuthorizationError::WorkspaceNotFound(_) => "workspace_not_found",
            AuthorizationError::WorkspaceArchived(_) => "workspace_archived",
            AuthorizationError::UnknownTemplate(_) => "authorization_template_unknown",
            AuthorizationError::DuplicateCapability(_) | AuthorizationError::MissingTarget(_) => "authorization_invalid",
            AuthorizationError::RiskNotAcknowledged => "risk_acknowledgement_required",
        }
    }
}

/// 整理使用者送來的授權內容：去掉前後空白與空白項目、確認每項能力只出現一次且有目標，
/// 並依能力的標準順序排列（讓「內容是否相同」可以直接比較）。
pub fn normalize_grants(grants: Vec<Grant>) -> Result<Vec<Grant>, AuthorizationError> {
    let clean = |items: Vec<String>| -> Vec<String> {
        items.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
    };
    let mut normalized: Vec<Grant> = Vec::with_capacity(grants.len());
    for grant in grants {
        if normalized.iter().any(|g| g.capability == grant.capability) {
            return Err(AuthorizationError::DuplicateCapability(grant.capability));
        }
        let targets = clean(grant.targets);
        if targets.is_empty() {
            return Err(AuthorizationError::MissingTarget(grant.capability));
        }
        normalized.push(Grant { capability: grant.capability, targets, restrictions: clean(grant.restrictions) });
    }
    normalized.sort_by_key(|g| g.capability);
    Ok(normalized)
}

/// 會執行專案或第三方依賴程式、或屬於高風險的能力，確認時必須先承認風險。
fn needs_risk_acknowledgement(grants: &[Grant]) -> bool {
    grants.iter().any(|g| g.capability.runs_trusted_code() || g.capability.is_high_risk())
}

/// 確認前給使用者看的摘要中的一項。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryItem {
    pub capability: Capability,
    pub label: String,
    pub targets: Vec<String>,
    pub restrictions: Vec<String>,
    pub high_risk: bool,
}

/// 確認摘要：逐項列出將生效的授權與必須揭露的風險。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmationSummary {
    pub items: Vec<SummaryItem>,
    pub risks: Vec<String>,
    /// 為真時，確認請求必須帶 `acknowledged_risk: true`。
    pub requires_risk_acknowledgement: bool,
}

pub fn summarize(grants: Vec<Grant>) -> Result<ConfirmationSummary, AuthorizationError> {
    let grants = normalize_grants(grants)?;
    let requires_risk_acknowledgement = needs_risk_acknowledgement(&grants);
    let mut risks = Vec::new();
    if requires_risk_acknowledgement {
        risks.push(TRUSTED_EXECUTION_RISK.to_string());
    }
    for g in grants.iter().filter(|g| g.capability.is_high_risk()) {
        risks.push(format!("「{}」不屬於可信專案開發範本，授權後 Agent 在範圍內不會再逐次詢問。", g.capability.label()));
    }
    let items = grants
        .into_iter()
        .map(|g| SummaryItem {
            label: g.capability.label().to_string(),
            high_risk: g.capability.is_high_risk(),
            capability: g.capability,
            targets: g.targets,
            restrictions: g.restrictions,
        })
        .collect();
    Ok(ConfirmationSummary { items, risks, requires_risk_acknowledgement })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmRequest {
    /// 以哪個範本為起點；省略表示使用者自行組成。
    pub template_id: Option<String>,
    pub grants: Vec<Grant>,
    pub acknowledged_risk: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmOutcome {
    /// 內容與目前生效的 revision 相同：不產生新的 revision。
    Unchanged(AuthorizationRevision),
    Created { revision: AuthorizationRevision, event: DomainEvent },
}

/// 確認並保存授權。`current` 是目前生效（最新）的 revision，尚未確認過則為 `None`。
pub fn confirm(
    workspace: &Workspace,
    current: Option<&AuthorizationRevision>,
    request: ConfirmRequest,
    now: Timestamp,
) -> Result<ConfirmOutcome, AuthorizationError> {
    if workspace.status == WorkspaceStatus::Archived {
        return Err(AuthorizationError::WorkspaceArchived(workspace.id.clone()));
    }
    let template = match request.template_id {
        None => None,
        Some(id) => Some(
            templates(&workspace.path)
                .into_iter()
                .find(|t| t.id == id)
                .ok_or(AuthorizationError::UnknownTemplate(id))?,
        ),
    };
    let grants = normalize_grants(request.grants)?;
    if needs_risk_acknowledgement(&grants) && !request.acknowledged_risk {
        return Err(AuthorizationError::RiskNotAcknowledged);
    }
    if let Some(current) = current.filter(|c| c.grants == grants) {
        return Ok(ConfirmOutcome::Unchanged(current.clone()));
    }

    let kind = match &template {
        None => SourceKind::Custom,
        Some(t) if normalize_grants(t.grants.clone())? == grants => SourceKind::Template,
        Some(_) => SourceKind::TemplateEdited,
    };
    let revision = AuthorizationRevision {
        workspace_id: workspace.id.clone(),
        revision: current.map_or(1, |c| c.revision + 1),
        grants,
        source: RevisionSource {
            kind,
            template_id: template.map(|t| t.id),
            base_revision: current.map(|c| c.revision),
            confirmed_by: CONFIRMED_BY_DESKTOP.to_string(),
        },
        confirmed_at: now,
    };
    let event = DomainEvent {
        aggregate_type: AGGREGATE_TYPE,
        aggregate_id: revision.workspace_id.clone(),
        aggregate_version: revision.revision,
        event_type: EVENT_CONFIRMED,
        occurred_at: now,
        payload: serde_json::json!({ "grants": revision.grants, "source": revision.source }),
    };
    Ok(ConfirmOutcome::Created { revision, event })
}

/// 准入檢查通過的結果：這次准入依據的授權 revision（不需要任何能力時為 `None`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Admission {
    pub authorization_revision: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionDenied {
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("Workspace「{0}」已經封存，不能建立 Run")]
    WorkspaceArchived(String),
    #[error("Workspace「{0}」還沒有確認授權，需要這些能力的 Run 不能被准入；請先在授權設定確認範本")]
    AuthorizationUnconfirmed(String),
    #[error("Workspace「{workspace}」目前的授權（revision {revision}）沒有包含：{}", labels(.missing))]
    CapabilityNotGranted { workspace: String, revision: i64, missing: Vec<Capability> },
}

fn labels(capabilities: &[Capability]) -> String {
    capabilities.iter().map(|c| format!("「{}」", c.label())).collect::<Vec<_>>().join("、")
}

impl AdmissionDenied {
    pub fn code(&self) -> &'static str {
        match self {
            AdmissionDenied::WorkspaceNotFound(_) => "workspace_not_found",
            AdmissionDenied::WorkspaceArchived(_) => "workspace_archived",
            AdmissionDenied::AuthorizationUnconfirmed(_) => "authorization_unconfirmed",
            AdmissionDenied::CapabilityNotGranted { .. } => "capability_not_granted",
        }
    }
}

/// Run 的准入檢查：Workspace 必須存在且使用中；`required` 裡的每個能力都必須被目前生效的 revision 授權。
/// 沒有任何 revision（授權尚未確認）時，只有「不需要任何能力」的 Run 能通過。
pub fn admit(
    workspace_id: &str,
    workspace: Option<&Workspace>,
    current: Option<&AuthorizationRevision>,
    required: &[Capability],
) -> Result<Admission, AdmissionDenied> {
    let workspace = workspace.ok_or_else(|| AdmissionDenied::WorkspaceNotFound(workspace_id.to_string()))?;
    if workspace.status == WorkspaceStatus::Archived {
        return Err(AdmissionDenied::WorkspaceArchived(workspace.name.clone()));
    }
    if required.is_empty() {
        return Ok(Admission { authorization_revision: current.map(|c| c.revision) });
    }
    let current = current.ok_or_else(|| AdmissionDenied::AuthorizationUnconfirmed(workspace.name.clone()))?;
    let mut missing: Vec<Capability> =
        required.iter().copied().filter(|c| !current.grants_capability(*c)).collect();
    missing.sort();
    missing.dedup();
    if !missing.is_empty() {
        return Err(AdmissionDenied::CapabilityNotGranted {
            workspace: workspace.name.clone(),
            revision: current.revision,
            missing,
        });
    }
    Ok(Admission { authorization_revision: Some(current.revision) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{decide, WorkspaceCommand};

    const T1: Timestamp = Timestamp::from_millis(1_000);
    const T2: Timestamp = Timestamp::from_millis(2_000);

    fn workspace() -> Workspace {
        decide(&[], WorkspaceCommand::Create { id: "ws_1".into(), name: None, path: "C:\\work\\app".into() }, T1)
            .unwrap()
            .workspace
    }

    fn template(id: &str) -> Template {
        templates("C:\\work\\app").into_iter().find(|t| t.id == id).unwrap()
    }

    fn request(template_id: Option<&str>, grants: Vec<Grant>, acknowledged: bool) -> ConfirmRequest {
        ConfirmRequest { template_id: template_id.map(Into::into), grants, acknowledged_risk: acknowledged }
    }

    fn created(outcome: ConfirmOutcome) -> (AuthorizationRevision, DomainEvent) {
        match outcome {
            ConfirmOutcome::Created { revision, event } => (revision, event),
            other => panic!("預期建立新 revision：{other:?}"),
        }
    }

    fn confirm_template(id: &str) -> AuthorizationRevision {
        let grants = template(id).grants;
        created(confirm(&workspace(), None, request(Some(id), grants, true), T1).unwrap()).0
    }

    fn capabilities(t: &Template) -> Vec<Capability> {
        t.grants.iter().map(|g| g.capability).collect()
    }

    #[test]
    fn trusted_dev_template_covers_edit_build_test_and_project_dependencies_only() {
        let caps = capabilities(&template(TEMPLATE_TRUSTED_DEV));

        for expected in [
            Capability::WriteFiles,
            Capability::RunBuildTest,
            Capability::InstallProjectDependencies,
        ] {
            assert!(caps.contains(&expected), "{expected:?}");
        }
        assert!(caps.iter().all(|c| !c.is_high_risk()), "P3 範本不得含管理員／全域安裝、遠端寫入、部署、對外發訊、付費、正式資料：{caps:?}");
    }

    #[test]
    fn no_template_contains_any_high_risk_capability() {
        for t in templates("C:\\work\\app") {
            assert!(capabilities(&t).iter().all(|c| !c.is_high_risk()), "{}", t.id);
        }
    }

    #[test]
    fn research_template_has_no_development_capabilities() {
        let caps = capabilities(&template(TEMPLATE_RESEARCH_DOCS));

        assert!(!caps.contains(&Capability::RunBuildTest));
        assert!(!caps.contains(&Capability::InstallProjectDependencies));
        assert!(caps.iter().all(|c| !c.runs_trusted_code()));
    }

    #[test]
    fn template_grants_name_capability_target_and_restrictions() {
        for t in templates("C:\\work\\app") {
            for g in &t.grants {
                assert!(!g.targets.is_empty(), "{}：{:?} 沒有目標資源", t.id, g.capability);
            }
        }
        let build = template(TEMPLATE_TRUSTED_DEV).grants.into_iter().find(|g| g.capability == Capability::RunBuildTest).unwrap();
        assert_eq!(build.targets, vec!["C:\\work\\app".to_string()], "目標資源代入 Workspace 目錄");
        assert!(!build.restrictions.is_empty());
    }

    #[test]
    fn exactly_the_seven_capabilities_outside_p3_are_high_risk() {
        let high: Vec<_> = Capability::ALL.into_iter().filter(|c| c.is_high_risk()).collect();

        assert_eq!(
            high,
            vec![
                Capability::AdminInstall,
                Capability::GlobalInstall,
                Capability::RemoteWrite,
                Capability::Deploy,
                Capability::ExternalMessage,
                Capability::Payment,
                Capability::ProductionData,
            ]
        );
    }

    #[test]
    fn first_confirmation_creates_revision_one_with_source_time_and_event() {
        let grants = template(TEMPLATE_TRUSTED_DEV).grants;

        let (revision, event) = created(
            confirm(&workspace(), None, request(Some(TEMPLATE_TRUSTED_DEV), grants.clone(), true), T2).unwrap(),
        );

        assert_eq!((revision.workspace_id.as_str(), revision.revision, revision.confirmed_at), ("ws_1", 1, T2));
        assert_eq!(revision.source.kind, SourceKind::Template);
        assert_eq!(revision.source.template_id.as_deref(), Some(TEMPLATE_TRUSTED_DEV));
        assert_eq!((revision.source.base_revision, revision.source.confirmed_by.as_str()), (None, "desktop"));
        assert_eq!(revision.grants, normalize_grants(grants).unwrap());
        assert_eq!((event.aggregate_type, event.event_type, event.aggregate_version), (AGGREGATE_TYPE, EVENT_CONFIRMED, 1));
        assert_eq!((event.aggregate_id.as_str(), event.occurred_at), ("ws_1", T2));
    }

    #[test]
    fn editing_a_confirmed_authorization_creates_the_next_revision_based_on_the_previous() {
        let first = confirm_template(TEMPLATE_TRUSTED_DEV);
        let mut edited = first.grants.clone();
        edited.retain(|g| g.capability != Capability::InstallProjectDependencies);

        let (second, event) = created(
            confirm(&workspace(), Some(&first), request(Some(TEMPLATE_TRUSTED_DEV), edited, true), T2).unwrap(),
        );

        assert_eq!((second.revision, second.confirmed_at), (2, T2));
        assert_eq!(second.source.kind, SourceKind::TemplateEdited);
        assert_eq!(second.source.base_revision, Some(1));
        assert_eq!(event.aggregate_version, 2);
        assert!(!second.grants_capability(Capability::InstallProjectDependencies));
    }

    #[test]
    fn confirming_without_a_template_is_custom() {
        let grants = vec![Grant { capability: Capability::ReadFiles, targets: vec!["C:\\work\\app".into()], restrictions: vec![] }];

        let (revision, _) = created(confirm(&workspace(), None, request(None, grants, false), T1).unwrap());

        assert_eq!((revision.source.kind, revision.source.template_id), (SourceKind::Custom, None));
    }

    #[test]
    fn identical_confirmation_does_not_create_a_new_revision() {
        let first = confirm_template(TEMPLATE_RESEARCH_DOCS);

        let outcome = confirm(&workspace(), Some(&first), request(Some(TEMPLATE_RESEARCH_DOCS), first.grants.clone(), false), T2)
            .unwrap();

        assert_eq!(outcome, ConfirmOutcome::Unchanged(first));
    }

    #[test]
    fn grant_order_and_surrounding_whitespace_do_not_make_a_new_revision() {
        let first = confirm_template(TEMPLATE_RESEARCH_DOCS);
        let mut shuffled = first.grants.clone();
        shuffled.reverse();
        shuffled[0].targets[0] = format!("  {}  ", shuffled[0].targets[0]);

        let outcome = confirm(&workspace(), Some(&first), request(None, shuffled, false), T2).unwrap();

        assert!(matches!(outcome, ConfirmOutcome::Unchanged(_)));
    }

    #[test]
    fn unknown_template_duplicate_capability_and_missing_target_are_rejected() {
        let ws = workspace();
        let read = |targets: Vec<&str>| Grant {
            capability: Capability::ReadFiles,
            targets: targets.into_iter().map(Into::into).collect(),
            restrictions: vec![],
        };

        assert_eq!(
            confirm(&ws, None, request(Some("nope"), vec![], false), T1),
            Err(AuthorizationError::UnknownTemplate("nope".into()))
        );
        assert_eq!(
            confirm(&ws, None, request(None, vec![read(vec!["a"]), read(vec!["b"])], false), T1),
            Err(AuthorizationError::DuplicateCapability(Capability::ReadFiles))
        );
        assert_eq!(
            confirm(&ws, None, request(None, vec![read(vec!["  "])], false), T1),
            Err(AuthorizationError::MissingTarget(Capability::ReadFiles))
        );
    }

    #[test]
    fn executing_trusted_code_requires_acknowledging_the_risk() {
        let grants = template(TEMPLATE_TRUSTED_DEV).grants;

        let result = confirm(&workspace(), None, request(Some(TEMPLATE_TRUSTED_DEV), grants, false), T1);

        assert_eq!(result, Err(AuthorizationError::RiskNotAcknowledged));
        assert!(AuthorizationError::RiskNotAcknowledged.to_string().contains("Windows 使用者的身分"));
    }

    #[test]
    fn a_high_risk_capability_alone_also_requires_acknowledging_the_risk() {
        let deploy = vec![Grant { capability: Capability::Deploy, targets: vec!["staging".into()], restrictions: vec![] }];

        assert_eq!(
            confirm(&workspace(), None, request(None, deploy.clone(), false), T1),
            Err(AuthorizationError::RiskNotAcknowledged)
        );
        assert!(confirm(&workspace(), None, request(None, deploy, true), T1).is_ok());
    }

    #[test]
    fn research_template_can_be_confirmed_without_the_risk_acknowledgement() {
        let grants = template(TEMPLATE_RESEARCH_DOCS).grants;

        assert!(confirm(&workspace(), None, request(Some(TEMPLATE_RESEARCH_DOCS), grants, false), T1).is_ok());
    }

    #[test]
    fn archived_workspace_cannot_be_authorized() {
        let mut ws = workspace();
        ws.status = WorkspaceStatus::Archived;

        let result = confirm(&ws, None, request(None, vec![], false), T1);

        assert_eq!(result, Err(AuthorizationError::WorkspaceArchived("ws_1".into())));
    }

    #[test]
    fn summary_of_trusted_dev_discloses_running_code_as_the_windows_user() {
        let summary = summarize(template(TEMPLATE_TRUSTED_DEV).grants).unwrap();

        assert!(summary.requires_risk_acknowledgement);
        assert!(summary.risks.iter().any(|r| r.contains("Windows 使用者的身分執行")), "{:?}", summary.risks);
        assert!(summary.items.iter().all(|i| !i.high_risk));
        assert_eq!(summary.items.len(), 4);
    }

    #[test]
    fn summary_of_research_template_has_no_execution_risk() {
        let summary = summarize(template(TEMPLATE_RESEARCH_DOCS).grants).unwrap();

        assert!(!summary.requires_risk_acknowledgement);
        assert!(summary.risks.is_empty());
    }

    #[test]
    fn summary_flags_each_explicitly_added_high_risk_capability() {
        let mut grants = template(TEMPLATE_TRUSTED_DEV).grants;
        grants.push(Grant { capability: Capability::Deploy, targets: vec!["staging".into()], restrictions: vec![] });

        let summary = summarize(grants).unwrap();

        let deploy = summary.items.iter().find(|i| i.capability == Capability::Deploy).unwrap();
        assert!(deploy.high_risk);
        assert!(summary.risks.iter().any(|r| r.contains("部署")));
    }

    // --- 准入 ---

    fn admit_with(current: Option<&AuthorizationRevision>, required: &[Capability]) -> Result<Admission, AdmissionDenied> {
        let ws = workspace();
        admit(&ws.id.clone(), Some(&ws), current, required)
    }

    #[test]
    fn run_requiring_a_capability_is_denied_while_authorization_is_unconfirmed() {
        let denied = admit_with(None, &[Capability::RunBuildTest]).unwrap_err();

        assert_eq!(denied, AdmissionDenied::AuthorizationUnconfirmed("app".into()));
        assert_eq!(denied.code(), "authorization_unconfirmed");
        assert!(denied.to_string().contains("還沒有確認授權"));
    }

    #[test]
    fn run_is_admitted_when_every_required_capability_is_granted_and_pins_the_revision() {
        let revision = confirm_template(TEMPLATE_TRUSTED_DEV);

        let admitted = admit_with(Some(&revision), &[Capability::WriteFiles, Capability::RunBuildTest]).unwrap();

        assert_eq!(admitted.authorization_revision, Some(1));
    }

    #[test]
    fn run_requiring_a_capability_outside_the_confirmed_authorization_is_denied_naming_it() {
        let revision = confirm_template(TEMPLATE_RESEARCH_DOCS);

        let denied = admit_with(Some(&revision), &[Capability::ReadFiles, Capability::RunBuildTest, Capability::Deploy]).unwrap_err();

        assert_eq!(
            denied,
            AdmissionDenied::CapabilityNotGranted {
                workspace: "app".into(),
                revision: 1,
                missing: vec![Capability::RunBuildTest, Capability::Deploy],
            }
        );
        assert_eq!(denied.code(), "capability_not_granted");
        assert!(denied.to_string().contains("建置與測試"));
    }

    #[test]
    fn run_without_required_capabilities_is_admitted_even_when_unconfirmed() {
        assert_eq!(admit_with(None, &[]).unwrap(), Admission { authorization_revision: None });
    }

    #[test]
    fn unknown_or_archived_workspace_is_denied() {
        let mut ws = workspace();

        assert_eq!(
            admit("ws_x", None, None, &[Capability::ReadFiles]),
            Err(AdmissionDenied::WorkspaceNotFound("ws_x".into()))
        );
        ws.status = WorkspaceStatus::Archived;
        assert_eq!(
            admit("ws_1", Some(&ws), None, &[]),
            Err(AdmissionDenied::WorkspaceArchived("app".into()))
        );
    }

    #[test]
    fn capability_names_are_stable_snake_case() {
        for capability in Capability::ALL {
            assert_eq!(serde_json::to_value(capability).unwrap(), capability.as_str());
        }
    }
}
