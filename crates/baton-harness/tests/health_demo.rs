//! 示範測試：透過 Runtime harness 的真實 HTTP API 驗證健康端點，
//! 並以「丟棄 Runtime、保留資料目錄後重建」模擬重啟（spec D-04）。

use baton_api::{HealthResponse, HEALTH_PATH};
use baton_domain::{product, Timestamp};
use baton_harness::Harness;

#[tokio::test]
async fn health_endpoint_reports_ok_over_real_http() {
    let harness = Harness::start().await;

    let health: HealthResponse = harness.get_json(HEALTH_PATH).await;

    assert_eq!(health.status, "ok");
    assert_eq!(health.product, product::PRODUCT_NAME);
    assert_eq!(health.boot_count, 1);
}

#[tokio::test]
async fn health_reports_the_pid_of_the_process_hosting_the_runtime() {
    let harness = Harness::start().await;

    let health: HealthResponse = harness.get_json(HEALTH_PATH).await;

    // harness 在測試程序內啟動 Runtime，所以回報的 pid 就是測試程序自己
    assert_eq!(health.pid, std::process::id());
}

#[tokio::test]
async fn health_started_at_comes_from_the_injected_clock() {
    let harness = Harness::builder().clock_at(Timestamp::from_millis(1_700_000_000_000)).start().await;

    let health: HealthResponse = harness.get_json(HEALTH_PATH).await;

    assert_eq!(health.started_at, Timestamp::from_millis(1_700_000_000_000));
}

#[tokio::test]
async fn runtime_still_responds_after_rebuild_and_remembers_previous_boot() {
    let mut harness = Harness::start().await;
    let before: HealthResponse = harness.get_json(HEALTH_PATH).await;

    harness.clock.advance_millis(60_000);
    harness.restart().await;

    let after: HealthResponse = harness.get_json(HEALTH_PATH).await;
    assert_eq!(after.status, "ok");
    // 資料目錄被保留：新的 Runtime 看得到上一次啟動的紀錄
    assert_eq!(after.boot_count, before.boot_count + 1);
    assert_eq!(after.started_at, before.started_at.plus_millis(60_000));
}

#[tokio::test]
async fn old_runtime_instance_is_gone_after_rebuild() {
    let mut harness = Harness::start().await;
    let old_url = harness.url(HEALTH_PATH);

    harness.restart().await;

    // 舊實例已被丟棄：原本的位址不再回應
    assert!(reqwest::get(old_url).await.is_err());
}

#[tokio::test]
async fn rebuild_reuses_the_same_fake_ports() {
    let mut harness = Harness::start().await;
    harness.quota_observer.set_sample(
        "profile-a",
        baton_ports::QuotaSample {
            source: baton_ports::QuotaSource::Observed,
            observed_at: Timestamp::from_millis(1),
            used: Some(0.5),
            reset_at: None,
            confidence: baton_ports::Confidence::Observed,
        },
    );

    harness.restart().await;

    // fake 的狀態不因重建而消失：腳本化的額度樣本仍在
    let sample = harness
        .ports()
        .quota_observer
        .observe(&baton_ports::ProfileRef("profile-a".into()))
        .await
        .unwrap();
    assert_eq!(sample.used, Some(0.5));
}

#[tokio::test]
async fn every_port_can_be_driven_through_its_fake() {
    use baton_ports::*;
    use std::time::Duration;

    let harness = Harness::start().await;
    let ports = harness.ports();

    // ProcessControl
    let receipt = ports
        .process_control
        .spawn(SpawnSpec { program: "claude".into(), args: vec![], working_dir: None, command_id: "cmd-1".into(), env: vec![], ..Default::default() })
        .await
        .unwrap();
    assert_eq!(ports.process_control.inspect(&receipt.identity).await.unwrap(), Liveness::Alive);
    harness.process_control.script_liveness(&receipt.identity, Liveness::Unknown);
    assert_eq!(ports.process_control.inspect(&receipt.identity).await.unwrap(), Liveness::Unknown);
    harness.process_control.set_stop_result(StopResult::StillAlive);
    assert_eq!(
        ports.process_control.stop(&receipt.identity, Duration::from_secs(30)).await.unwrap(),
        StopResult::StillAlive
    );
    assert_eq!(harness.process_control.spawn_count(), 1);

    // AgentAdapter
    harness.agent_adapter.script_events("run-1", vec![AdapterEvent::Exited { success: true }]);
    let handle = ports
        .agent_adapter
        .launch(RunSpec {
            run_id: "run-1".into(),
            task_id: "task-1".into(),
            command_id: "cmd-1".into(),
            prompt: "hi".into(),
            working_dir: None,
            ..Default::default()
        })
        .await
        .unwrap();
    let events: Vec<_> = futures::StreamExt::collect(ports.agent_adapter.events(&handle)).await;
    assert_eq!(events, vec![AdapterEvent::Exited { success: true }]);
    harness.agent_adapter.set_capability(Capability::NotResumable);
    let capability = ports
        .agent_adapter
        .resume_capability(&SessionRef { native_session_id: "s".into(), config_dir: None, working_dir: None })
        .await
        .unwrap();
    assert_eq!(capability, Capability::NotResumable);

    // WorkspaceFs
    let target = harness.data_dir().join("wt");
    ports.workspace_fs.create_worktree(harness.data_dir(), "branch", &target).await.unwrap();
    harness.workspace_fs.mark_dirty(&target);
    assert_eq!(ports.workspace_fs.inspect_dirty(&target).await.unwrap(), DirtyState::Dirty);
    ports.workspace_fs.remove_worktree(&target).await.unwrap();
    let artifact = harness.data_dir().join("artifacts").join("a.txt");
    ports.workspace_fs.write_artifact_atomic(&artifact, b"data").await.unwrap();
    assert_eq!(std::fs::read(&artifact).unwrap(), b"data");

    // PlatformInfo
    harness.platform_info.set(PlatformSnapshot {
        os_family: OsFamily::Windows11,
        os_build: "26300".into(),
        feature_version: "26H2".into(),
        edition: "Pro".into(),
        native_arch: "x64".into(),
        process_arch: "x64".into(),
    });
    assert_eq!(ports.platform_info.snapshot().os_build, "26300");
}
