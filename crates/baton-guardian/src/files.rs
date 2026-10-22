//! 監護與 Runtime 之間的檔案（spec D-24、D-25）：監護日誌、恢復狀態鏡像、生命週期檔與兩者的程序身分。
//!
//! 監護只寫監護日誌（append，每筆 fsync）與自己的程序身分；恢復狀態鏡像與 Runtime 的程序身分由 Runtime 寫。
//! 檔案都在 `<資料目錄>\guardian\`，生命週期檔在資料目錄本身。

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use baton_domain::guardian::{
    self, JournalRecord, LifecycleFileRead, RecoveryMirror, GUARDIAN_DIR, GUARDIAN_INSTANCE_FILE_NAME, JOURNAL_FILE_NAME,
    RECOVERY_MIRROR_FILE_NAME, RUNTIME_INSTANCE_FILE_NAME,
};
use baton_domain::lifecycle::{LifecycleFile, LIFECYCLE_FILE_NAME};
use baton_domain::Timestamp;
use serde::{Deserialize, Serialize};

/// 監護日誌的診斷文字檔（給人看，不參與任何判斷）。
pub const GUARDIAN_LOG_FILE_NAME: &str = "guardian.log";

pub fn guardian_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(GUARDIAN_DIR)
}

pub fn journal_path(data_dir: &Path) -> PathBuf {
    guardian_dir(data_dir).join(JOURNAL_FILE_NAME)
}

pub fn recovery_mirror_path(data_dir: &Path) -> PathBuf {
    guardian_dir(data_dir).join(RECOVERY_MIRROR_FILE_NAME)
}

pub fn runtime_instance_path(data_dir: &Path) -> PathBuf {
    guardian_dir(data_dir).join(RUNTIME_INSTANCE_FILE_NAME)
}

pub fn guardian_instance_path(data_dir: &Path) -> PathBuf {
    guardian_dir(data_dir).join(GUARDIAN_INSTANCE_FILE_NAME)
}

/// 在監護日誌追加一筆紀錄並 fsync。前一筆寫到一半（沒有換行）時先補換行，免得兩筆黏成一行。
pub fn append_journal(data_dir: &Path, record: &JournalRecord) -> std::io::Result<()> {
    std::fs::create_dir_all(guardian_dir(data_dir))?;
    let mut file = std::fs::OpenOptions::new().read(true).append(true).create(true).open(journal_path(data_dir))?;
    let mut line = String::new();
    if file.metadata()?.len() > 0 {
        let mut last = [0u8; 1];
        file.seek(SeekFrom::End(-1))?;
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            line.push('\n');
        }
    }
    line.push_str(&record.to_line());
    line.push('\n');
    file.write_all(line.as_bytes())?;
    file.sync_all()
}

/// 讀取監護日誌；還沒有日誌時是空的。
pub fn read_journal(data_dir: &Path) -> std::io::Result<Vec<JournalRecord>> {
    match std::fs::read(journal_path(data_dir)) {
        Ok(bytes) => Ok(guardian::parse_journal(&String::from_utf8_lossy(&bytes))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// 原子寫入：暫存檔 → fsync → 改名取代。
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    {
        let mut out = std::fs::File::create(&temp)?;
        out.write_all(bytes)?;
        out.sync_all()?;
    }
    std::fs::rename(&temp, path)
}

/// Runtime 寫入恢復狀態鏡像。
pub fn write_recovery_mirror(data_dir: &Path, mirror: &RecoveryMirror) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(mirror).map_err(std::io::Error::other)?;
    write_atomic(&recovery_mirror_path(data_dir), &bytes)
}

/// 讀取恢復狀態鏡像；沒有或讀不懂都視為沒有（計次仍以監護日誌與 Runtime 的控制資料庫為準）。
pub fn read_recovery_mirror(data_dir: &Path) -> Option<RecoveryMirror> {
    let bytes = std::fs::read(recovery_mirror_path(data_dir)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 讀取生命週期檔：沒有檔案、讀得懂、或讀不懂（停止意圖不明）。
pub fn read_lifecycle(data_dir: &Path) -> LifecycleFileRead {
    match std::fs::read(data_dir.join(LIFECYCLE_FILE_NAME)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LifecycleFileRead::Missing,
        Err(_) => LifecycleFileRead::Unreadable,
        Ok(bytes) => match serde_json::from_slice::<LifecycleFile>(&bytes) {
            Ok(file) => LifecycleFileRead::Parsed(file),
            Err(_) => LifecycleFileRead::Unreadable,
        },
    }
}

/// 程序身分：程序識別碼與程序啟動後記下的時間。程序識別碼可能被重複使用，所以以「程序建立時間不晚於記錄時間」確認是同一個程序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceRecord {
    pub pid: u32,
    pub recorded_at: Timestamp,
}

pub fn write_instance(path: &Path, record: &InstanceRecord) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    write_atomic(path, &bytes)
}

pub fn read_instance(path: &Path) -> Option<InstanceRecord> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// 記下程序身分的那個程序是否還在執行。
pub fn instance_alive(path: &Path) -> bool {
    read_instance(path).is_some_and(|record| crate::process::alive_since(record.pid, record.recorded_at))
}

/// 追加一行診斷文字（失敗就算了，不影響任何判斷）。
pub fn append_log(data_dir: &Path, now: Timestamp, message: &str) {
    let _ = std::fs::create_dir_all(guardian_dir(data_dir));
    if let Ok(mut file) =
        std::fs::OpenOptions::new().append(true).create(true).open(guardian_dir(data_dir).join(GUARDIAN_LOG_FILE_NAME))
    {
        let _ = writeln!(file, "{} {message}", now.as_millis());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use baton_domain::guardian::GuardianAttempt;

    fn record(id: &str) -> JournalRecord {
        JournalRecord::Attempt(GuardianAttempt {
            event_id: "rec_1".into(),
            round: 1,
            seq: 1,
            attempt_id: id.into(),
            detected_at: Timestamp::from_millis(1),
            at: Timestamp::from_millis(2),
            reason: "測試".into(),
        })
    }

    #[test]
    fn journal_appends_survive_a_torn_previous_line() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_journal(dir.path()).unwrap().is_empty(), "還沒有日誌");
        append_journal(dir.path(), &record("a")).unwrap();
        // 模擬上一次寫到一半就中斷
        let mut file = std::fs::OpenOptions::new().append(true).open(journal_path(dir.path())).unwrap();
        file.write_all(b"{\"kind\":\"attempt\",\"eve").unwrap();
        drop(file);
        append_journal(dir.path(), &record("b")).unwrap();
        let ids: Vec<String> = read_journal(dir.path()).unwrap().iter().map(|r| r.attempt_id().to_string()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn corrupt_lifecycle_file_is_unreadable_and_missing_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_lifecycle(dir.path()), LifecycleFileRead::Missing);
        std::fs::write(dir.path().join(LIFECYCLE_FILE_NAME), b"{not json").unwrap();
        assert_eq!(read_lifecycle(dir.path()), LifecycleFileRead::Unreadable);
    }

    #[test]
    fn recovery_mirror_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_recovery_mirror(dir.path()).is_none());
        let mirror = RecoveryMirror::of(None, Some("rcv_1".into()), Timestamp::from_millis(5));
        write_recovery_mirror(dir.path(), &mirror).unwrap();
        assert_eq!(read_recovery_mirror(dir.path()), Some(mirror));
    }
}
