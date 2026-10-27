use baton_domain::profile::ProfileCommand;

use super::*;

const T1: Timestamp = Timestamp::from_millis(1_000);

fn create(id: &str) -> ProfileCommand {
    ProfileCommand::Create {
        id: id.into(),
        name: id.into(),
        config_dir: format!("C:\\baton\\profiles\\{id}"),
        expected_account: None,
    }
}

async fn store_with(dir: &std::path::Path, ids: &[&str]) -> Store {
    let store = Store::open(dir, T1).await.unwrap();
    for id in ids {
        store.apply_profile_command(create(id), T1).await.unwrap();
    }
    store
}

fn order(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn profiles_are_shown_in_creation_order_until_reordered() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with(dir.path(), &["p1", "p2", "p3"]).await;

    assert_eq!(store.profile_display_order().await.unwrap(), order(&["p1", "p2", "p3"]));
}

#[tokio::test]
async fn a_saved_order_survives_reopen_and_does_not_change_the_creation_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with(dir.path(), &["p1", "p2", "p3"]).await;

    store.reorder_profiles(order(&["p3", "p1", "p2"])).await.unwrap();
    drop(store);
    let reopened = Store::open(dir.path(), T1).await.unwrap();

    assert_eq!(reopened.profile_display_order().await.unwrap(), order(&["p3", "p1", "p2"]));
    // 派工與重複帳號的判定仍依建立順序
    let created: Vec<String> = reopened.list_profiles().await.unwrap().into_iter().map(|p| p.id).collect();
    assert_eq!(created, order(&["p1", "p2", "p3"]));
}

#[tokio::test]
async fn a_new_profile_goes_last_and_later_updates_keep_its_place() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with(dir.path(), &["p1", "p2"]).await;
    store.reorder_profiles(order(&["p2", "p1"])).await.unwrap();

    store.apply_profile_command(create("p3"), T1).await.unwrap();
    store.apply_profile_command(ProfileCommand::Archive { id: "p2".into() }, T1).await.unwrap();

    assert_eq!(store.profile_display_order().await.unwrap(), order(&["p2", "p1", "p3"]));
}

#[tokio::test]
async fn archived_profiles_follow_the_active_ones_after_a_reorder() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with(dir.path(), &["p1", "p2", "p3"]).await;
    store.apply_profile_command(ProfileCommand::Archive { id: "p1".into() }, T1).await.unwrap();

    store.reorder_profiles(order(&["p3", "p2"])).await.unwrap();

    assert_eq!(store.profile_display_order().await.unwrap(), order(&["p3", "p2", "p1"]));
}

#[tokio::test]
async fn an_order_that_does_not_match_the_active_profiles_is_rejected_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with(dir.path(), &["p1", "p2"]).await;

    let error = store.reorder_profiles(order(&["p2"])).await.unwrap_err();

    assert!(matches!(error, ProfileApplyError::Rejected(ProfileError::OrderMismatch)), "{error:?}");
    assert_eq!(store.profile_display_order().await.unwrap(), order(&["p1", "p2"]));
}
