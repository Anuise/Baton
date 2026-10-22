//! 桌面生命週期（票 25）：關閉視窗後 Run 持續、重開視窗連回同一個 Runtime 且不重複啟動 Worker；
//! 以及桌面殼經由 RuntimeClient 完全退出、變更自動續跑與「繼續工作」（與 App 實際使用的路徑相同）。
//!
//! 「關閉視窗」對 Runtime 而言就是桌面 App 的連線消失：App 只是 Runtime 的用戶端，Runtime 是獨立的程序。

use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

use baton_api::{CreateTaskRequest, CreateWorkspaceRequest};
use baton_client::{ensure_runtime, ClientError, Connection, DispatchOutcome, RuntimeClient};
use baton_domain::lifecycle::LifecyclePhase;
use baton_domain::run::RunStatus;
use baton_domain::task::{ContractContent, ContractField};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

fn filled(text: &str) -> Option<ContractField> {
    Some(ContractField::Filled { text: text.into(), references: vec![] })
}

fn complete() -> ContractContent {
    ContractContent {
        objective: filled("整理資料"),
        inputs: Some(ContractField::NotApplicable { reason: "沒有輸入".into() }),
        scope: filled("公開資料"),
        deliverables: filled("比較表"),
        done_criteria: filled("五個產品"),
        blocker_handling: filled("回報"),
        required_capabilities: vec![],
    }
}

async fn new_workspace(harness: &Harness, client: &RuntimeClient) -> String {
    let dir = harness.data_dir().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    client
        .create_workspace(&CreateWorkspaceRequest { path: dir.to_string_lossy().into_owned(), name: None })
        .await
        .unwrap()
        .id
}

/// 建立一個 ready 的 Task 並派工，回傳 Run 識別。
async fn dispatched_run(client: &RuntimeClient, workspace_id: &str) -> String {
    let goal = client.create_goal(workspace_id, "研究競品").await.unwrap();
    let created = client
        .create_task(workspace_id, &CreateTaskRequest { goal_id: goal.id, title: "整理".into(), contract: complete() })
        .await
        .unwrap();
    client.mark_task_ready(workspace_id, &created.task.id).await.unwrap();
    match client.dispatch_task(workspace_id, &created.task.id).await.unwrap() {
        DispatchOutcome::Started(run) => run.run.id,
        DispatchOutcome::Queued(queued) => panic!("應該直接派出，卻排隊中：{queued:?}"),
    }
}

/// App 開啟時的連線流程：連回既有的 Runtime，沒有才啟動；回傳連線方式與啟動次數。
async fn open_app(client: &RuntimeClient) -> (Connection, u64, usize) {
    let started = Cell::new(0);
    let (connection, health) = ensure_runtime(
        client,
        || {
            started.set(started.get() + 1);
            Ok(())
        },
        Duration::from_millis(500),
    )
    .await
    .unwrap();
    (connection, health.boot_count, started.get())
}

#[tokio::test]
async fn closing_the_window_keeps_runs_going_and_reopening_connects_to_the_same_runtime_without_another_worker() {
    let harness = Harness::start().await;
    harness.add_account("帳號", "a@example.com").await;
    let first_window = app_client(&harness);
    let (_, boot_count, _) = open_app(&first_window).await;
    let workspace_id = new_workspace(&harness, &first_window).await;
    let run_id = dispatched_run(&first_window, &workspace_id).await;
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 1);

    // 關閉視窗：App 的連線消失，Runtime 與 Run 不受影響
    drop(first_window);
    harness.supervise().await;
    harness.supervise().await;
    assert!(harness.process_control.stop_calls().is_empty(), "關閉視窗不會停止任何 Run");

    // 重新開啟視窗：連回同一個 Runtime，不啟動第二個控制平面
    let second_window = app_client(&harness);
    let (connection, boot_count_again, spawned_runtimes) = open_app(&second_window).await;
    assert_eq!(connection, Connection::AlreadyRunning);
    assert_eq!(spawned_runtimes, 0);
    assert_eq!(boot_count_again, boot_count, "同一個 Runtime：啟動次數沒有增加");

    harness.supervise().await;
    let run = second_window.task_board(&workspace_id).await.unwrap().runs.into_iter().find(|r| r.run.id == run_id).unwrap();
    assert_eq!(run.run.status, RunStatus::Running, "Run 持續執行");
    assert_eq!(harness.process_control.spawn_count(), 1, "重開視窗不會重複啟動 Worker");
    let lifecycle = second_window.lifecycle().await.unwrap();
    assert_eq!(lifecycle.phase, LifecyclePhase::Running);
    assert_eq!(lifecycle.active_runs, 1);
}

#[tokio::test]
async fn the_desktop_requests_full_exit_and_manages_workspace_resume_through_the_client() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;

    let view = client.set_workspace_auto_resume(&workspace_id, true).await.unwrap();
    assert!(view.auto_resume_after_exit);
    assert!(!view.awaiting_resume);

    let accepted = client.request_exit().await.unwrap();
    assert!(accepted.stop_intent_active);
    harness.wait_for_exit().await;
    let status = client.lifecycle().await.unwrap();
    assert_eq!(status.phase, LifecyclePhase::Stopped);
    assert!(status.exit.unwrap().completed_at.is_some());

    // 停止意圖解除（重新開啟）之前不能「繼續工作」
    match client.resume_workspace(&workspace_id).await {
        Err(ClientError::Rejected { code, .. }) => assert_eq!(code, "stop_intent_active"),
        other => panic!("應該被拒絕：{other:?}"),
    }
}
