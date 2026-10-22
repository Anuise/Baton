//! 生命週期持久化的單元測試（票 25）：生命週期檔的原子讀寫與保守判讀。

use baton_domain::lifecycle::{FileStopView, LifecycleFile, StopSuppression, LIFECYCLE_FILE_VERSION};

use super::*;

#[test]
fn lifecycle_file_round_trips_and_replaces_the_previous_content() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(read_lifecycle_file(dir.path()), FileStopView::Missing);

    let stop = LifecycleFile {
        version: LIFECYCLE_FILE_VERSION,
        stop: Some(StopSuppression { requested_at: Timestamp::from_millis(7), reason: "full_exit".into() }),
        maintenance: None,
        updated_at: Timestamp::from_millis(7),
    };
    write_lifecycle_file(dir.path(), &stop).unwrap();
    assert_eq!(read_lifecycle_file(dir.path()), FileStopView::Stop { requested_at: Some(Timestamp::from_millis(7)) });

    let clear = LifecycleFile { stop: None, updated_at: Timestamp::from_millis(9), ..stop };
    write_lifecycle_file(dir.path(), &clear).unwrap();
    assert_eq!(read_lifecycle_file(dir.path()), FileStopView::Clear);
    assert!(!dir.path().join(format!("{LIFECYCLE_FILE_NAME}.tmp")).exists(), "暫存檔已改名取代");
}

#[test]
fn a_corrupted_lifecycle_file_is_read_as_a_stop_intent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(lifecycle_file_path(dir.path()), b"{ not json").unwrap();
    assert_eq!(read_lifecycle_file(dir.path()), FileStopView::Stop { requested_at: None });
}
