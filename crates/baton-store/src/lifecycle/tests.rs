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

#[cfg(windows)]
#[test]
fn writers_and_readers_survive_other_programs_briefly_opening_the_lifecycle_file() {
    // 最終 review 修正（R029）：上一個測試在整包加壓時偶發 os error 5。原因是防毒、索引器等外部程式會短暫開啟剛寫好的檔案，
    // 而且不允許刪除共享（沒有 FILE_SHARE_DELETE）；這時改名取代會被拒。這裡以不允許刪除共享的開檔模擬外部程式，
    // 同時讓讀者並行讀取：取代進行中的瞬間，讀者也可能遇到「找不到檔案」或「存取被拒」（舊檔待刪），不能因此誤判
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};

    let dir = tempfile::tempdir().unwrap();
    let file = LifecycleFile {
        version: LIFECYCLE_FILE_VERSION,
        stop: Some(StopSuppression { requested_at: Timestamp::from_millis(7), reason: "full_exit".into() }),
        maintenance: None,
        updated_at: Timestamp::from_millis(7),
    };
    write_lifecycle_file(dir.path(), &file).unwrap();
    let expected = FileStopView::Stop { requested_at: Some(Timestamp::from_millis(7)) };
    let done = AtomicBool::new(false);
    let (failures, misreads): (Vec<String>, Vec<FileStopView>) = std::thread::scope(|scope| {
        // 外部程式：以「讀取＋寫入共享、不含刪除共享」開啟目標檔約 1 毫秒、放開約 2 毫秒，反覆進行（開不了就算了）。
        // 不能不間斷地開：那會讓目標檔在高負載下幾乎一直被占用，模擬的就不是「短暫」開啟了
        scope.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                let held = std::fs::OpenOptions::new().read(true).share_mode(0x1 | 0x2).open(lifecycle_file_path(dir.path()));
                std::thread::sleep(std::time::Duration::from_millis(1));
                drop(held);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        });
        // 執行緒數刻意壓低：整包加壓時一起跑的測試已經占滿 CPU，這裡再多開忙碌的執行緒只會讓上面的「短暫」開啟被拖長
        let reader = scope.spawn(|| {
            let mut misreads = Vec::new();
            while !done.load(Ordering::Relaxed) {
                let view = read_lifecycle_file(dir.path());
                if view != expected {
                    misreads.push(view);
                }
            }
            misreads
        });
        let writers: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| (0..50).filter_map(|_| write_lifecycle_file(dir.path(), &file).err()).map(|e| e.to_string()).collect::<Vec<_>>()))
            .collect();
        let failures = writers.into_iter().flat_map(|w| w.join().unwrap()).collect();
        done.store(true, Ordering::Relaxed);
        (failures, reader.join().unwrap())
    });
    assert!(failures.is_empty(), "{} 次寫入失敗，例如：{:?}", failures.len(), failures.first());
    assert!(misreads.is_empty(), "{} 次讀錯，例如：{:?}", misreads.len(), misreads.first());
    let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(leftovers, [std::ffi::OsString::from(LIFECYCLE_FILE_NAME)], "不留下暫存檔");
}
