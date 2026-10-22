//! 桌面殼經由 RuntimeClient 設定 Workspace 授權（票 06）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。

use std::sync::Arc;

use baton_api::{AdmissionCheckRequest, AuthorizationStatus, ConfirmAuthorizationRequest, CreateWorkspaceRequest, SummaryRequest};
use baton_client::{ClientError, RuntimeClient};
use baton_domain::authorization::{Capability, SourceKind, TEMPLATE_TRUSTED_DEV};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

#[tokio::test]
async fn app_confirms_the_p3_template_and_only_then_runs_needing_build_are_admitted() {
    let mut harness = Harness::start().await;
    let dir = harness.data_dir().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let client = app_client(&harness);
    let workspace = client
        .create_workspace(&CreateWorkspaceRequest { path: dir.to_string_lossy().into_owned(), name: None })
        .await
        .unwrap();
    let needs_build = AdmissionCheckRequest { required_capabilities: vec![Capability::RunBuildTest] };

    let before = client.authorization(&workspace.id).await.unwrap();
    let refused = client.check_admission(&workspace.id, &needs_build).await.unwrap_err();
    let grants = before.templates.iter().find(|t| t.id == TEMPLATE_TRUSTED_DEV).unwrap().grants.clone();
    let summary = client.authorization_summary(&workspace.id, &SummaryRequest { grants: grants.clone() }).await.unwrap();
    let revision = client
        .confirm_authorization(
            &workspace.id,
            &ConfirmAuthorizationRequest {
                template_id: Some(TEMPLATE_TRUSTED_DEV.into()),
                grants,
                acknowledged_risk: true,
                acknowledged_expansion: false,
            },
        )
        .await
        .unwrap();

    assert_eq!(before.status, AuthorizationStatus::Unconfirmed);
    assert!(matches!(&refused, ClientError::Rejected { code, .. } if code == "authorization_unconfirmed"), "{refused:?}");
    assert!(summary.requires_risk_acknowledgement);
    assert_eq!((revision.revision, revision.source.kind), (1, SourceKind::Template));
    assert_eq!(client.check_admission(&workspace.id, &needs_build).await.unwrap().authorization_revision, Some(1));

    harness.restart().await;
    let client = app_client(&harness);
    let after = client.authorization(&workspace.id).await.unwrap();
    assert_eq!((after.status, after.effective), (AuthorizationStatus::Confirmed, Some(revision)));
}
