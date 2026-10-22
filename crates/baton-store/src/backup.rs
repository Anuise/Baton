//! 備份、還原與還原演練（票 40，spec D-18）。
//!
//! 版面（都在資料目錄的 `backups/` 之下）：
//! - `<備份識別>/control.sqlite`：以 SQLite Online Backup API 取得的一致快照，已移除憑證資料表的內容並 VACUUM。
//! - `<備份識別>/manifest.json`：快照的雜湊，以及快照中登記的全部成果版本（路徑、大小、SHA-256）。
//! - `blobs/<SHA-256>`：成果檔案的副本，以內容雜湊命名；成果不可變，所以多份備份共用同一個副本。
//! - `drill.json`：最新一次還原演練的結果。
//!
//! 備份先寫在 `<備份識別>.staging`，manifest 寫完才改名成正式名稱，所以列得出來的備份都是完整的。
//! 備份只含控制資料庫與成果：Profile 的原生設定目錄（登入資料）、桌面憑證檔、Run 設定快照與日誌都不在其中；
//! 資料庫中的 `credential`（憑證雜湊與範圍）也在快照中清空，還原後啟動 Runtime 時會重新簽發桌面憑證。
//!
//! 還原只寫入乾淨的目錄。還原後，備份當下尚未結束的 Run 一律結束（已啟動的標為中斷、還沒啟動的取消），
//! 讓新的 Runtime 不會重新啟動程序或重送任何外部操作；需要的話由使用者重新派工。

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use baton_domain::artifact::{sha256_hex, ARTIFACTS_DIR};
use baton_domain::backup::{
    self, ArtifactFileEntry, BackupManifest, BackupTrigger, DrillCheck, DrillCheckKind, DrillReport,
};
use baton_domain::run::{RunCommand, RunStatus};

use super::runs::{decide_in, load_active_runs, save};
use super::tasks::load;
use super::*;

/// 資料目錄下的備份子目錄（spec D-17）。
pub const BACKUPS_DIR: &str = "backups";
const BLOBS_DIR: &str = "blobs";
const MANIFEST_FILE: &str = "manifest.json";
const DRILL_FILE: &str = "drill.json";
const STAGING_SUFFIX: &str = ".staging";
/// 還原演練的隔離目錄；每次演練前後都整個刪除。
const DRILL_WORK_DIR: &str = ".drill";

/// 還原時把已啟動的 Run 標為中斷的原因。
pub const RESTORE_INTERRUPT_REASON: &str = "restored_from_backup：備份當下尚未結束，還原後不自動重新啟動";

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("{0}")]
    Io(String),
    #[error("資料庫錯誤：{0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("還原目標 {0} 不是空目錄；只能還原到乾淨的目錄")]
    TargetNotEmpty(PathBuf),
    #[error("{0} 不是有效的備份：{1}")]
    InvalidBackup(PathBuf, String),
    #[error("還原時無法結束 Run {0}：{1}")]
    Neutralize(String, String),
    #[error("備份工作被中斷")]
    Join,
}

pub type BackupResult<T> = Result<T, BackupError>;

fn io(what: &str, path: &Path) -> impl FnOnce(std::io::Error) -> BackupError {
    let message = format!("{what} {}", path.display());
    move |e| BackupError::Io(format!("{message}：{e}"))
}

/// 還原的結果：被還原的備份、核對結果，以及被結束的 Run 數。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    pub manifest: BackupManifest,
    pub checks: Vec<DrillCheck>,
    pub interrupted_runs: usize,
}

impl RestoreReport {
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|c| c.passed)
    }
}

/// 資料目錄的備份根目錄。
pub fn backups_root(data_dir: &Path) -> PathBuf {
    data_dir.join(BACKUPS_DIR)
}

impl Store {
    /// 建立一份備份：在唯讀連線上取得一致快照（只在複製資料庫頁面的期間佔用該連線，不擋寫入），再複製被引用的成果。
    pub async fn create_backup(&self, trigger: BackupTrigger, now: Timestamp) -> BackupResult<BackupManifest> {
        let reader = self.inner.reader.clone();
        let data_dir = self.inner.data_dir.clone();
        tokio::task::spawn_blocking(move || {
            let staging = begin_staging(&data_dir, trigger, now)?;
            {
                let source = reader.lock().unwrap_or_else(|e| e.into_inner());
                snapshot(&source, &staging.db_path())?;
            }
            staging.finish(&data_dir, trigger, now)
        })
        .await
        .map_err(|_| BackupError::Join)?
    }
}

/// 套用 migration 之前（Store 開啟資料庫時）的備份；來源是還沒有其他連線的寫入連線。
pub(crate) fn backup_before_migration(source: &Connection, data_dir: &Path, now: Timestamp) -> BackupResult<BackupManifest> {
    let staging = begin_staging(data_dir, BackupTrigger::PreMigration, now)?;
    snapshot(source, &staging.db_path())?;
    staging.finish(data_dir, BackupTrigger::PreMigration, now)
}

/// 寫到一半的備份目錄。
struct Staging {
    id: String,
    dir: PathBuf,
}

impl Staging {
    fn db_path(&self) -> PathBuf {
        self.dir.join(DB_FILE_NAME)
    }

    /// 清除快照中的憑證、複製成果、寫入 manifest，最後改名成正式的備份目錄。
    fn finish(self, data_dir: &Path, trigger: BackupTrigger, now: Timestamp) -> BackupResult<BackupManifest> {
        let db_path = self.db_path();
        scrub_secrets(&db_path)?;
        let (schema_version, artifacts) = {
            let conn = Connection::open(&db_path)?;
            (schema_version_of(&conn)?, artifact_entries(&conn)?)
        };
        let blobs = backups_root(data_dir).join(BLOBS_DIR);
        let artifacts_root = data_dir.join(ARTIFACTS_DIR);
        for entry in &artifacts {
            store_blob(&artifacts_root.join(&entry.storage_path), &blobs, entry)?;
        }
        let database = std::fs::read(&db_path).map_err(io("無法讀取快照", &db_path))?;
        let manifest = BackupManifest {
            id: self.id.clone(),
            trigger,
            created_at: now,
            schema_version,
            database_size_bytes: database.len() as i64,
            database_sha256: sha256_hex(&database),
            artifacts,
        };
        let json = serde_json::to_vec_pretty(&manifest).expect("manifest 可序列化");
        write_atomic(&self.dir.join(MANIFEST_FILE), &json)?;
        let target = backups_root(data_dir).join(&self.id);
        std::fs::rename(&self.dir, &target).map_err(io("無法完成備份", &target))?;
        Ok(manifest)
    }
}

/// 建立空的暫存備份目錄；同一毫秒、同一原因已經有備份時加上序號。
fn begin_staging(data_dir: &Path, trigger: BackupTrigger, now: Timestamp) -> BackupResult<Staging> {
    let root = backups_root(data_dir);
    let base = backup::backup_id(trigger, now);
    let id = (1..)
        .map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
        .find(|id| !root.join(id).exists())
        .expect("序號用不完");
    let dir = root.join(format!("{id}{STAGING_SUFFIX}"));
    remove_dir_if_exists(&dir)?;
    std::fs::create_dir_all(&dir).map_err(io("無法建立備份目錄", &dir))?;
    Ok(Staging { id, dir })
}

/// SQLite Online Backup API：一次複製全部頁面，期間持有讀取交易，所以得到的是同一時間點的一致快照。
fn snapshot(source: &Connection, target: &Path) -> BackupResult<()> {
    let mut destination = Connection::open(target)?;
    let backup = rusqlite::backup::Backup::new(source, &mut destination)?;
    match backup.step(-1)? {
        rusqlite::backup::StepResult::Done => Ok(()),
        other => Err(BackupError::Io(format!("資料庫快照沒有完成：{other:?}"))),
    }
}

fn table_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)", [name], |r| r.get(0))
}

/// 快照不保存任何憑證：清空 `credential` 並 VACUUM，讓被刪除的內容不殘留在空閒頁面。
fn scrub_secrets(db_path: &Path) -> BackupResult<()> {
    let conn = Connection::open(db_path)?;
    // 不用 WAL：之後直接讀取整個檔案計算雜湊，內容不能留在另外的 -wal 檔裡
    let _: String = conn.pragma_update_and_check(None, "journal_mode", "DELETE", |r| r.get(0))?;
    if table_exists(&conn, "credential")? {
        conn.execute("DELETE FROM credential", [])?;
    }
    conn.execute_batch("VACUUM")?;
    Ok(())
}

fn schema_version_of(conn: &Connection) -> BackupResult<usize> {
    if !table_exists(conn, "schema_migrations")? {
        return Ok(0);
    }
    Ok(applied_version(conn)?)
}

/// 快照中登記的全部成果版本（還沒有成果資料表的舊版資料庫沒有任何成果）。
fn artifact_entries(conn: &Connection) -> BackupResult<Vec<ArtifactFileEntry>> {
    if !table_exists(conn, "artifact_version")? {
        return Ok(Vec::new());
    }
    let entries = conn
        .prepare("SELECT storage_path, size_bytes, sha256 FROM artifact_version ORDER BY storage_path")?
        .query_map([], |r| Ok(ArtifactFileEntry { storage_path: r.get(0)?, size_bytes: r.get(1)?, sha256: r.get(2)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(entries)
}

/// 把成果複製成以雜湊命名的副本；已經有內容相符的副本就沿用（成果不可變），損毀的副本以成果檔案重寫。
/// 成果檔案不見了或內容與 manifest 不符時不複製：備份照常完成，還原演練會把它報告為雜湊不一致。
fn store_blob(source: &Path, blobs: &Path, entry: &ArtifactFileEntry) -> BackupResult<()> {
    let matches = |bytes: &[u8]| bytes.len() as i64 == entry.size_bytes && sha256_hex(bytes) == entry.sha256;
    let target = blobs.join(&entry.sha256);
    if std::fs::read(&target).is_ok_and(|existing| matches(&existing)) {
        return Ok(());
    }
    match std::fs::read(source) {
        Ok(bytes) if matches(&bytes) => write_atomic(&target, &bytes),
        _ => Ok(()),
    }
}

/// 先寫暫存、fsync、再改名（已存在的目標會被取代）。
fn write_atomic(path: &Path, bytes: &[u8]) -> BackupResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io("無法建立目錄", parent))?;
    }
    let mut temp_name = path.file_name().unwrap_or_default().to_os_string();
    temp_name.push(".partial");
    let temp = path.with_file_name(temp_name);
    let mut file = std::fs::File::create(&temp).map_err(io("無法建立暫存檔", &temp))?;
    file.write_all(bytes).map_err(io("無法寫入暫存檔", &temp))?;
    file.sync_all().map_err(io("無法 fsync 暫存檔", &temp))?;
    drop(file);
    std::fs::rename(&temp, path).map_err(io("無法改名為", path))
}

fn remove_dir_if_exists(dir: &Path) -> BackupResult<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io("無法移除", dir)(e)),
    }
}

fn read_manifest(dir: &Path) -> BackupResult<BackupManifest> {
    let path = dir.join(MANIFEST_FILE);
    let bytes = std::fs::read(&path).map_err(|e| BackupError::InvalidBackup(dir.to_path_buf(), e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| BackupError::InvalidBackup(dir.to_path_buf(), e.to_string()))
}

/// 已完成的備份，由舊到新；寫到一半的（`.staging`）與讀不到 manifest 的目錄不算。
pub fn list_backups(data_dir: &Path) -> BackupResult<Vec<BackupManifest>> {
    let root = backups_root(data_dir);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io("無法列出", &root)(e)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io("無法列出", &root))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(STAGING_SUFFIX) || !entry.path().is_dir() {
            continue;
        }
        if let Ok(manifest) = read_manifest(&entry.path()) {
            if manifest.id == name {
                found.push(manifest);
            }
        }
    }
    found.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    Ok(found)
}

/// 依保留規則刪除多餘的備份與不再被引用的成果副本，回傳被刪除的備份識別。
/// 也清掉先前中斷留下的 `.staging` 目錄（備份一律依序進行，這時不會有別的備份正在寫）。
pub fn apply_retention(data_dir: &Path) -> BackupResult<Vec<String>> {
    let root = backups_root(data_dir);
    let backups = list_backups(data_dir)?;
    let kept = backup::retention(&backups);
    let mut removed = Vec::new();
    for manifest in &backups {
        if !kept.contains_key(&manifest.id) {
            remove_dir_if_exists(&root.join(&manifest.id))?;
            removed.push(manifest.id.clone());
        }
    }
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().ends_with(STAGING_SUFFIX) {
                remove_dir_if_exists(&entry.path())?;
            }
        }
    }
    let referenced: HashSet<String> = backups
        .iter()
        .filter(|m| kept.contains_key(&m.id))
        .flat_map(|m| m.artifacts.iter().map(|a| a.sha256.clone()))
        .collect();
    let blobs = root.join(BLOBS_DIR);
    if let Ok(entries) = std::fs::read_dir(&blobs) {
        for entry in entries.flatten() {
            if !referenced.contains(&*entry.file_name().to_string_lossy()) {
                std::fs::remove_file(entry.path()).map_err(io("無法移除", &entry.path()))?;
            }
        }
    }
    Ok(removed)
}

/// 把 `backup_dir`（`backups/<備份識別>`）還原到乾淨的 `target` 資料目錄，並核對還原結果。
///
/// 核對：資料庫快照的雜湊、`integrity_check`、`foreign_key_check`、每個成果檔案的大小與雜湊、資料庫登記的成果版本與 manifest 一致。
/// 資料庫完整時接著套用 migration，並結束備份當下尚未結束的 Run（不自動重做外部操作）。
/// 核對沒通過不是錯誤（結果在 `checks` 裡）；只有前提不成立（目標不是空目錄、備份讀不到、備份的 schema 比程式新、
/// manifest 的路徑會跑出還原目錄）或 I/O 失敗才回傳錯誤。
pub fn restore_backup(backup_dir: &Path, target: &Path, now: Timestamp) -> BackupResult<RestoreReport> {
    let manifest = read_manifest(backup_dir)?;
    if manifest.schema_version > MIGRATIONS.len() {
        return Err(StoreError::IncompatibleSchema { found: manifest.schema_version, supported: MIGRATIONS.len() }.into());
    }
    for entry in &manifest.artifacts {
        check_manifest_entry(backup_dir, entry)?;
    }
    if std::fs::read_dir(target).is_ok_and(|mut entries| entries.next().is_some()) {
        return Err(BackupError::TargetNotEmpty(target.to_path_buf()));
    }
    std::fs::create_dir_all(target).map_err(io("無法建立還原目錄", target))?;

    let source_db = backup_dir.join(DB_FILE_NAME);
    let database = std::fs::read(&source_db).map_err(io("無法讀取備份的資料庫", &source_db))?;
    let db_intact = sha256_hex(&database) == manifest.database_sha256;
    let db_path = target.join(DB_FILE_NAME);
    write_atomic(&db_path, &database)?;

    let blobs = backup_dir.parent().map(|p| p.join(BLOBS_DIR)).unwrap_or_default();
    let artifacts_root = target.join(ARTIFACTS_DIR);
    let mut restored = BTreeMap::new();
    for entry in &manifest.artifacts {
        let Ok(bytes) = std::fs::read(blobs.join(&entry.sha256)) else { continue };
        write_atomic(&artifacts_root.join(&entry.storage_path), &bytes)?;
        let path = artifacts_root.join(&entry.storage_path);
        let written = std::fs::read(&path).map_err(io("無法讀取還原的成果", &path))?;
        restored.insert(entry.storage_path.clone(), (written.len() as i64, sha256_hex(&written)));
    }

    let mut conn = Connection::open(&db_path)?;
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    let broken_relations = conn.prepare("PRAGMA foreign_key_check")?.query_map([], |_| Ok(()))?.count();
    let recorded = artifact_entries(&conn)?;

    let mut checks = vec![
        DrillCheck {
            kind: DrillCheckKind::Restore,
            passed: db_intact,
            detail: if db_intact {
                format!("已還原到乾淨目錄：資料庫快照與 {} 個成果檔案", restored.len())
            } else {
                "備份中的資料庫快照與 manifest 記錄的雜湊不一致".to_string()
            },
        },
        DrillCheck {
            kind: DrillCheckKind::DatabaseIntegrity,
            passed: integrity == "ok",
            detail: format!("integrity_check：{integrity}"),
        },
        DrillCheck {
            kind: DrillCheckKind::ForeignKeys,
            passed: broken_relations == 0,
            detail: if broken_relations == 0 {
                "資料表之間的關聯完整".to_string()
            } else {
                format!("有 {broken_relations} 筆資料參照到不存在的資料")
            },
        },
    ];
    checks.extend(backup::check_artifacts(&manifest.artifacts, &recorded, &restored));

    let interrupted_runs = if integrity == "ok" {
        // manifest 可能與快照不符：以快照實際的版本為準，較新就不套用 migration、不改寫任何資料
        ensure_compatible(&conn)?;
        migrate(&mut conn, now, None)?;
        end_unfinished_runs(&mut conn, now)?
    } else {
        0
    };
    Ok(RestoreReport { manifest, checks, interrupted_runs })
}

/// 成果路徑只能是還原目錄底下的相對路徑（不含 `..`、根目錄或磁碟代號），成果副本的雜湊只能是 64 位十六進位檔名。
fn check_manifest_entry(backup_dir: &Path, entry: &ArtifactFileEntry) -> BackupResult<()> {
    use std::path::Component;
    let path = Path::new(&entry.storage_path);
    let relative = !entry.storage_path.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)));
    if !relative {
        let message = format!("成果路徑 {} 不在成果目錄內", entry.storage_path);
        return Err(BackupError::InvalidBackup(backup_dir.to_path_buf(), message));
    }
    if entry.sha256.len() != 64 || !entry.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        let message = format!("成果 {} 的雜湊 {} 格式不正確", entry.storage_path, entry.sha256);
        return Err(BackupError::InvalidBackup(backup_dir.to_path_buf(), message));
    }
    Ok(())
}

/// 結束備份當下尚未結束的 Run：已啟動的標為中斷、還沒啟動的取消；兩者都釋放寫入所有權並讓 Task 回到 ready。
/// 同一個交易中把快照裡最後一次（備份當下）尚未結束的啟動記為正常結束：還原不是崩潰，還原後第一次啟動不觸發崩潰恢復。
fn end_unfinished_runs(conn: &mut Connection, now: Timestamp) -> BackupResult<usize> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_compatible(&tx)?;
    tx.execute(
        "UPDATE runtime_boot SET ended_cleanly_at = ?1
         WHERE id = (SELECT MAX(id) FROM runtime_boot) AND ended_cleanly_at IS NULL",
        [now.as_millis()],
    )?;
    let mut ended = 0;
    for run in load_active_runs(&tx)? {
        let command = if run.status == RunStatus::Queued {
            RunCommand::RequestCancel { run_id: run.id.clone() }
        } else {
            RunCommand::Interrupt { run_id: run.id.clone(), reason: RESTORE_INTERRUPT_REASON.to_string() }
        };
        let loaded = load(&tx, &run.workspace_id)?;
        let change = decide_in(&loaded, &run.workspace_id, &loaded.tasks, command, now)
            .map_err(|e| BackupError::Neutralize(run.id.clone(), e.to_string()))?;
        if change.changed() {
            save(&tx, &change, now)?;
            ended += 1;
        }
    }
    tx.commit()?;
    Ok(ended)
}

/// 還原演練：把最新的備份還原到隔離目錄、核對，記下結果後刪除隔離目錄。沒有任何備份時不演練，回傳 `None`。
pub fn run_drill(data_dir: &Path, now: Timestamp) -> BackupResult<Option<DrillReport>> {
    let backups = list_backups(data_dir)?;
    let Some(latest) = backups.last() else { return Ok(None) };
    let root = backups_root(data_dir);
    let work = root.join(DRILL_WORK_DIR);
    remove_dir_if_exists(&work)?;
    let report = match restore_backup(&root.join(&latest.id), &work, now) {
        Ok(restored) => DrillReport::new(Some(latest), now, restored.checks, restored.interrupted_runs),
        Err(error) => {
            let failed = DrillCheck { kind: DrillCheckKind::Restore, passed: false, detail: error.to_string() };
            DrillReport::new(Some(latest), now, vec![failed], 0)
        }
    };
    remove_dir_if_exists(&work)?;
    let json = serde_json::to_vec_pretty(&report).expect("演練結果可序列化");
    write_atomic(&root.join(DRILL_FILE), &json)?;
    Ok(Some(report))
}

/// 最新一次還原演練的結果；還沒做過時為 `None`。
pub fn latest_drill(data_dir: &Path) -> BackupResult<Option<DrillReport>> {
    let path = backups_root(data_dir).join(DRILL_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| BackupError::Io(format!("無法解讀 {}：{e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io("無法讀取", &path)(e)),
    }
}

#[cfg(test)]
mod tests;
