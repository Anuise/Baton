//! 使用者貼上 Anthropic API 金鑰更新模型設定的可用清單（AI-33）：經由 RuntimeClient，與 App 實際使用的路徑相同。
//! Anthropic 由 harness 的 fake 取代，測試不會連線。

use std::path::Path;
use std::sync::Arc;

use baton_api::ModelCatalog;
use baton_client::{ClientError, RuntimeClient};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;
use baton_ports::{Clock, ModelDirectoryError};

const KEY: &str = "sk-ant-api03-test-key-0123456789";
const ALIASES: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

fn ids(catalog: &ModelCatalog) -> Vec<&str> {
    catalog.models.iter().map(|m| m.id.as_str()).collect()
}

/// 資料目錄下（資料庫、日誌、保存的清單…）任何檔案都不含金鑰。
fn assert_key_not_stored(dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert_key_not_stored(&path);
        } else if let Ok(bytes) = std::fs::read(&path) {
            let found = bytes.windows(KEY.len()).any(|window| window == KEY.as_bytes());
            assert!(!found, "{} 含有 API 金鑰", path.display());
        }
    }
}

/// 第一頁兩個模型、第二頁一個模型。
fn script_two_pages(harness: &Harness) {
    harness.model_directory.respond_page(
        &[("claude-opus-5-5", "Claude Opus 5.5"), ("claude-sonnet-5-5", "Claude Sonnet 5.5")],
        Some("claude-sonnet-5-5"),
    );
    harness.model_directory.respond_page(&[("claude-haiku-4-5-20251001", "Claude Haiku 4.5")], None);
}

#[tokio::test]
async fn before_any_refresh_the_builtin_list_is_used() {
    let harness = Harness::start().await;

    let catalog = app_client(&harness).available_models().await.unwrap();

    assert_eq!(catalog.updated_at, None);
    assert_eq!(&ids(&catalog)[..4], ALIASES);
    assert!(harness.model_directory.calls().is_empty());
}

#[tokio::test]
async fn refresh_reads_every_page_keeps_the_aliases_and_the_list_survives_a_runtime_rebuild() {
    let mut harness = Harness::start().await;
    script_two_pages(&harness);
    let client = app_client(&harness);

    let refreshed = client.refresh_models(KEY).await.unwrap();

    // 第二頁以第一頁的 last_id 當 after_id
    assert_eq!(
        harness.model_directory.calls(),
        vec![(KEY.to_string(), None), (KEY.to_string(), Some("claude-sonnet-5-5".to_string()))]
    );
    assert_eq!(
        ids(&refreshed),
        ["fable", "opus", "sonnet", "haiku", "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-4-5-20251001"]
    );
    assert_eq!(refreshed.models[4].label, "Claude Opus 5.5");
    assert_eq!(refreshed.updated_at, Some(harness.clock.now()));
    assert_eq!(client.available_models().await.unwrap(), refreshed);

    harness.restart().await;
    assert_eq!(app_client(&harness).available_models().await.unwrap(), refreshed);
    // 保存的是清單與更新時間，不含金鑰
    assert!(harness.data_dir().join("model-catalog.json").is_file());
    assert_key_not_stored(harness.data_dir());
}

#[tokio::test]
async fn a_rejected_key_keeps_the_previous_list_and_the_error_does_not_contain_the_key() {
    let harness = Harness::start().await;
    script_two_pages(&harness);
    let client = app_client(&harness);
    let previous = client.refresh_models(KEY).await.unwrap();

    harness.model_directory.respond(Err(ModelDirectoryError::KeyRejected(401)));
    let error = client.refresh_models(KEY).await.unwrap_err();

    assert!(matches!(&error, ClientError::Rejected { code, .. } if code == "model_api_key_rejected"), "{error:?}");
    assert!(error.to_string().contains("401"), "{error}");
    assert!(!error.to_string().contains(KEY), "{error}");
    assert_eq!(client.available_models().await.unwrap(), previous);
    assert_key_not_stored(harness.data_dir());
}

#[tokio::test]
async fn a_network_failure_keeps_the_builtin_list_and_masks_the_key_even_if_the_cause_mentions_it() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let builtin = client.available_models().await.unwrap();

    harness.model_directory.respond(Err(ModelDirectoryError::Unreachable(format!("連線逾時（{KEY}）"))));
    let error = client.refresh_models(KEY).await.unwrap_err();

    assert!(matches!(&error, ClientError::Rejected { code, .. } if code == "model_directory_unavailable"), "{error:?}");
    assert!(error.to_string().contains("連線逾時"), "{error}");
    assert!(!error.to_string().contains(KEY), "{error}");
    assert_eq!(client.available_models().await.unwrap(), builtin);
    assert_key_not_stored(harness.data_dir());
}

#[tokio::test]
async fn a_failure_on_a_later_page_saves_nothing() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    harness.model_directory.respond_page(&[("claude-opus-5-5", "Claude Opus 5.5")], Some("claude-opus-5-5"));
    harness.model_directory.respond(Err(ModelDirectoryError::Status(529)));

    let error = client.refresh_models(KEY).await.unwrap_err();

    assert!(matches!(&error, ClientError::Rejected { code, .. } if code == "model_directory_unavailable"), "{error:?}");
    assert_eq!(client.available_models().await.unwrap().updated_at, None);
}

#[tokio::test]
async fn an_empty_key_is_rejected_without_asking_anthropic() {
    let harness = Harness::start().await;

    let error = app_client(&harness).refresh_models("   ").await.unwrap_err();

    assert!(matches!(&error, ClientError::Rejected { code, .. } if code == "model_api_key_missing"), "{error:?}");
    assert!(harness.model_directory.calls().is_empty());
}
