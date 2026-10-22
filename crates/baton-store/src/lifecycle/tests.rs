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

#[test]
fn concurrent_writers_of_the_lifecycle_file_do_not_trip_over_each_other() {
    // API 的 request_exit 與 Supervisor 的鏡像可能同時寫入（最終 code-review 修正）：各自的暫存檔不能互相覆寫或被搶先改名
    let dir = tempfile::tempdir().unwrap();
    let file = LifecycleFile {
        version: LIFECYCLE_FILE_VERSION,
        stop: Some(StopSuppression { requested_at: Timestamp::from_millis(7), reason: "full_exit".into() }),
        maintenance: None,
        updated_at: Timestamp::from_millis(7),
    };
    let failures: Vec<String> = std::thread::scope(|scope| {
        let writers: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| (0..50).filter_map(|_| write_lifecycle_file(dir.path(), &file).err()).map(|e| e.to_string()).collect::<Vec<_>>()))
            .collect();
        writers.into_iter().flat_map(|w| w.join().unwrap()).collect()
    });
    assert!(failures.is_empty(), "{} 次寫入失敗，例如：{:?}", failures.len(), failures.first());
    assert_eq!(read_lifecycle_file(dir.path()), FileStopView::Stop { requested_at: Some(Timestamp::from_millis(7)) });
    let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(leftovers, [std::ffi::OsString::from(LIFECYCLE_FILE_NAME)], "不留下暫存檔");
}
