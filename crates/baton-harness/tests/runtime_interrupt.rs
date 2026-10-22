//! 正式執行檔的結束方式（票 31；AI-280、review-notes #13）：主控台 Ctrl-C 或服務自行結束（完全退出後）都是有秩序的停止，
//! 記下正常結束，下次啟動不判為崩潰；只有程序被終止、當掉或重開機才算意外終止。

use baton_domain::Timestamp;
use baton_harness::fakes::FakePorts;
use baton_runtime::{Runtime, RuntimeConfig, RuntimeOptions};

fn options() -> RuntimeOptions {
    RuntimeOptions {
        background_supervisor: false,
        background_backup: false,
        background_update_check: false,
        ..RuntimeOptions::default()
    }
}

/// 最近一次啟動是否記下了正常結束。
fn last_boot_ended_cleanly(data_dir: &std::path::Path) -> bool {
    let db = rusqlite::Connection::open(data_dir.join(baton_store::DB_FILE_NAME)).unwrap();
    db.query_row("SELECT ended_cleanly_at IS NOT NULL FROM runtime_boot ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
        .unwrap()
}

#[tokio::test]
async fn a_console_interrupt_stops_the_runtime_in_order_and_is_not_a_crash() {
    let data_dir = tempfile::tempdir().unwrap();
    let fakes = FakePorts::new(Timestamp::from_millis(1_000_000_000_000));
    let config = RuntimeConfig { data_dir: data_dir.path().to_path_buf(), bind: "127.0.0.1:0".parse().unwrap() };
    let runtime = Runtime::start_with(config, fakes.to_ports(), options()).await.unwrap();

    // 主控台按下 Ctrl-C
    runtime.serve_until(std::future::ready(())).await;

    assert!(last_boot_ended_cleanly(data_dir.path()), "Ctrl-C 是有秩序的停止，下次啟動不應觸發崩潰恢復");
}
