//! `baton-runtime restore` 失敗時的訊息（票 40；review-notes #5）：說的是還原失敗，不是 Runtime 啟動失敗。

use std::process::Command;

const RUNTIME_EXE: &str = env!("CARGO_BIN_EXE_baton-runtime");

#[test]
fn a_failed_restore_reports_a_restore_failure_not_a_startup_failure() {
    let dir = tempfile::tempdir().unwrap();
    let missing_backup = dir.path().join("no-such-backup");
    let target = dir.path().join("restored");

    let output = Command::new(RUNTIME_EXE).arg("restore").arg(&missing_backup).arg(&target).output().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("還原失敗："), "{stderr}");
    assert!(!stderr.contains("Runtime 啟動失敗"), "{stderr}");
}
