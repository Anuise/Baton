use super::*;
use baton_domain::auth::{CredentialRecord, Role};

const DAY: i64 = 24 * 60 * 60 * 1000;
const T0: Timestamp = Timestamp::from_millis(1_792_368_000_000);

async fn open(dir: &Path) -> Store {
    Store::open(dir, T0).await.unwrap()
}

fn credential(hash: &str) -> CredentialRecord {
    CredentialRecord { token_hash: hash.into(), role: Role::Desktop, issued_at: T0, expires_at: None, scope: None, controller: None }
}

/// 備份目錄下全部檔案的位元組（掃描用）。
fn all_bytes(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(dir: &Path, found: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            if entry.path().is_dir() {
                walk(&entry.path(), found);
            } else {
                found.push((entry.path(), std::fs::read(entry.path()).unwrap()));
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, &mut found);
    found
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[tokio::test]
async fn backup_is_a_consistent_snapshot_without_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    let secret_hash = "c0ffee".repeat(10);
    store.issue_credential(credential(&secret_hash)).await.unwrap();
    store.record_boot(T0).await.unwrap();

    let manifest = store.create_backup(BackupTrigger::Daily, T0).await.unwrap();

    let backup_dir = backups_root(dir.path()).join(&manifest.id);
    let snapshot = Connection::open(backup_dir.join(DB_FILE_NAME)).unwrap();
    let boots: i64 = snapshot.query_row("SELECT COUNT(*) FROM runtime_boot", [], |r| r.get(0)).unwrap();
    assert_eq!(boots, 1, "快照要包含備份當下已提交的資料");
    let credentials: i64 = snapshot.query_row("SELECT COUNT(*) FROM credential", [], |r| r.get(0)).unwrap();
    assert_eq!(credentials, 0);
    drop(snapshot);
    for (path, bytes) in all_bytes(&backups_root(dir.path())) {
        assert!(!contains(&bytes, &secret_hash), "{} 含有憑證雜湊", path.display());
    }
    assert_eq!(manifest.trigger, BackupTrigger::Daily);
    assert_eq!(manifest.schema_version, MIGRATIONS.len());
    assert_eq!(list_backups(dir.path()).unwrap(), vec![manifest]);
    // 原本的資料庫不受影響
    assert!(store.find_credential(&secret_hash).await.unwrap().is_some());
}

#[tokio::test]
async fn backups_created_in_the_same_millisecond_get_distinct_ids() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    let first = store.create_backup(BackupTrigger::Milestone, T0).await.unwrap();
    let second = store.create_backup(BackupTrigger::Milestone, T0).await.unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(list_backups(dir.path()).unwrap().len(), 2);
}

#[tokio::test]
async fn unfinished_staging_directories_are_not_backups_and_are_cleaned_by_retention() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    let manifest = store.create_backup(BackupTrigger::Daily, T0).await.unwrap();
    // 模擬寫到一半中斷：連 manifest 都有了，但還沒改名
    let staging = backups_root(dir.path()).join(format!("9999999999999-daily{STAGING_SUFFIX}"));
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::copy(backups_root(dir.path()).join(&manifest.id).join(MANIFEST_FILE), staging.join(MANIFEST_FILE)).unwrap();

    assert_eq!(list_backups(dir.path()).unwrap().len(), 1);
    apply_retention(dir.path()).unwrap();
    assert!(!staging.exists());
}

#[tokio::test]
async fn retention_removes_expired_backups_and_unreferenced_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    for day in 0..40 {
        store.create_backup(BackupTrigger::Daily, T0.plus_millis(day * DAY)).await.unwrap();
    }
    let stray = backups_root(dir.path()).join(BLOBS_DIR).join("0".repeat(64));
    std::fs::create_dir_all(stray.parent().unwrap()).unwrap();
    std::fs::write(&stray, b"x").unwrap();

    let removed = apply_retention(dir.path()).unwrap();

    let remaining = list_backups(dir.path()).unwrap();
    assert_eq!(remaining.len() + removed.len(), 40);
    // 7 個每日（第 33～39 天）；每週的四份中第 39、34 天已在其中，另外加上第 27、20 天
    assert_eq!(remaining.len(), backup::KEEP_DAILY + 2);
    assert!(remaining.iter().all(|m| !removed.contains(&m.id)));
    assert!(!stray.exists(), "沒有任何備份引用的成果副本要刪除");
}

#[tokio::test]
async fn restore_only_writes_into_a_clean_directory() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    let manifest = store.create_backup(BackupTrigger::Daily, T0).await.unwrap();
    let target = tempfile::tempdir().unwrap();
    std::fs::write(target.path().join("existing"), b"keep").unwrap();

    let result = restore_backup(&backups_root(dir.path()).join(&manifest.id), target.path(), T0);

    assert!(matches!(result, Err(BackupError::TargetNotEmpty(_))), "{result:?}");
    assert_eq!(std::fs::read(target.path().join("existing")).unwrap(), b"keep");
}

#[tokio::test]
async fn restored_database_passes_every_check_and_opens_as_a_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    store.record_boot(T0).await.unwrap();
    let manifest = store.create_backup(BackupTrigger::Daily, T0).await.unwrap();
    let target = tempfile::tempdir().unwrap();
    let restored_dir = target.path().join("restored");

    let report = restore_backup(&backups_root(dir.path()).join(&manifest.id), &restored_dir, T0).unwrap();

    assert!(report.passed(), "{:?}", report.checks);
    assert_eq!(report.interrupted_runs, 0);
    let restored = Store::open(&restored_dir, T0).await.unwrap();
    assert_eq!(restored.record_boot(T0).await.unwrap(), 2, "還原後延續備份當下的資料");
}

#[tokio::test]
async fn tampered_snapshot_fails_the_restore_check() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    let manifest = store.create_backup(BackupTrigger::Daily, T0).await.unwrap();
    let backup_dir = backups_root(dir.path()).join(&manifest.id);
    let mut tampered = read_manifest(&backup_dir).unwrap();
    tampered.database_sha256 = "f".repeat(64);
    std::fs::write(backup_dir.join(MANIFEST_FILE), serde_json::to_vec(&tampered).unwrap()).unwrap();

    let report = run_drill(dir.path(), T0).unwrap().unwrap();

    assert!(!report.passed);
    assert!(report.checks.iter().any(|c| c.kind == DrillCheckKind::Restore && !c.passed), "{:?}", report.checks);
    assert_eq!(latest_drill(dir.path()).unwrap(), Some(report));
    assert!(!backups_root(dir.path()).join(DRILL_WORK_DIR).exists(), "演練的隔離目錄用完就刪除");
}

#[tokio::test]
async fn drill_without_any_backup_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let _store = open(dir.path()).await;
    assert_eq!(run_drill(dir.path(), T0).unwrap(), None);
    assert_eq!(latest_drill(dir.path()).unwrap(), None);
}

// --- migration 前的備份 ---

/// 建立只套用了前 `applied` 個 migration 的資料庫（模擬舊版程式留下的資料）。
fn older_database(dir: &Path, applied: usize) {
    let conn = Connection::open(dir.join(DB_FILE_NAME)).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at INTEGER NOT NULL);",
    )
    .unwrap();
    for (index, migration) in MIGRATIONS.iter().enumerate().take(applied) {
        conn.execute_batch(migration.sql).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, 1)",
            rusqlite::params![index + 1, migration.name],
        )
        .unwrap();
    }
    conn.execute("INSERT INTO runtime_boot (started_at) VALUES (7)", []).unwrap();
}

#[tokio::test]
async fn opening_an_older_database_backs_it_up_before_migrating() {
    let dir = tempfile::tempdir().unwrap();
    let older = MIGRATIONS.len() - 1;
    older_database(dir.path(), older);

    let _store = open(dir.path()).await;

    let backups = list_backups(dir.path()).unwrap();
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].trigger, BackupTrigger::PreMigration);
    assert_eq!(backups[0].schema_version, older, "備份的是 migration 之前的版本");
    let snapshot = Connection::open(backups_root(dir.path()).join(&backups[0].id).join(DB_FILE_NAME)).unwrap();
    let boots: i64 = snapshot.query_row("SELECT COUNT(*) FROM runtime_boot", [], |r| r.get(0)).unwrap();
    assert_eq!(boots, 1);
}

#[tokio::test]
async fn new_and_current_databases_are_not_backed_up_on_open() {
    let dir = tempfile::tempdir().unwrap();
    drop(open(dir.path()).await);
    drop(open(dir.path()).await);
    assert!(list_backups(dir.path()).unwrap().is_empty());
}

#[tokio::test]
async fn failed_pre_migration_backup_leaves_the_database_unmigrated() {
    let dir = tempfile::tempdir().unwrap();
    let older = MIGRATIONS.len() - 1;
    older_database(dir.path(), older);
    // 備份目錄的位置被一個檔案佔住，備份一定失敗
    std::fs::write(backups_root(dir.path()), b"not a directory").unwrap();

    let result = Store::open(dir.path(), T0).await;

    assert!(matches!(result, Err(StoreError::PreMigrationBackup(_))), "{:?}", result.err());
    let conn = Connection::open(dir.path().join(DB_FILE_NAME)).unwrap();
    assert_eq!(applied_version(&conn).unwrap(), older);
}

#[tokio::test]
async fn first_start_after_restoring_a_regular_backup_is_not_a_crash() {
    // 一般備份的快照中，備份當下那次啟動還沒正常結束（R151 第 15 項）
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    store.record_boot(T0).await.unwrap();
    let manifest = store.create_backup(BackupTrigger::Daily, T0).await.unwrap();
    let target = tempfile::tempdir().unwrap();
    let restored_dir = target.path().join("restored");

    restore_backup(&backups_root(dir.path()).join(&manifest.id), &restored_dir, T0).unwrap();

    let restored = Store::open(&restored_dir, T0).await.unwrap();
    restored.record_boot(T0).await.unwrap();
    assert_eq!(restored.previous_boot_ended_cleanly().await.unwrap(), Some(true));
}

/// 改寫備份 manifest（測試用）。
fn rewrite_manifest(backup_dir: &Path, change: impl FnOnce(&mut BackupManifest)) {
    let mut manifest = read_manifest(backup_dir).unwrap();
    change(&mut manifest);
    std::fs::write(backup_dir.join(MANIFEST_FILE), serde_json::to_vec(&manifest).unwrap()).unwrap();
}

#[tokio::test]
async fn a_backup_from_a_newer_schema_is_not_restored() {
    // R151 第 18 項：舊版程式不能改寫新版 schema 的資料
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    let manifest = store.create_backup(BackupTrigger::Daily, T0).await.unwrap();
    let backup_dir = backups_root(dir.path()).join(&manifest.id);

    // manifest 宣告較新的 schema
    rewrite_manifest(&backup_dir, |m| m.schema_version = MIGRATIONS.len() + 1);
    let target = tempfile::tempdir().unwrap();
    let result = restore_backup(&backup_dir, &target.path().join("a"), T0);
    assert!(matches!(result, Err(BackupError::Store(StoreError::IncompatibleSchema { .. }))), "{result:?}");
    assert!(!target.path().join("a").exists(), "被拒絕時不寫入任何檔案");

    // manifest 照舊，但快照本身被較新的版本升級過
    let snapshot = backup_dir.join(DB_FILE_NAME);
    let conn = Connection::open(&snapshot).unwrap();
    conn.execute("INSERT INTO schema_migrations (version, name, applied_at) VALUES (999, 'future', 0)", []).unwrap();
    drop(conn);
    let sha = sha256_hex(&std::fs::read(&snapshot).unwrap());
    rewrite_manifest(&backup_dir, |m| {
        m.schema_version = MIGRATIONS.len();
        m.database_sha256 = sha;
    });
    let result = restore_backup(&backup_dir, &target.path().join("b"), T0);
    assert!(matches!(result, Err(BackupError::Store(StoreError::IncompatibleSchema { found: 999, .. }))), "{result:?}");
}

#[tokio::test]
async fn manifest_paths_that_escape_the_restore_target_are_rejected() {
    // review-notes #5：storage_path 不能帶 `..` 或絕對路徑；成果副本的雜湊也只能是檔名
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    let manifest = store.create_backup(BackupTrigger::Daily, T0).await.unwrap();
    let backup_dir = backups_root(dir.path()).join(&manifest.id);
    let blob = b"escaped";
    let sha = sha256_hex(blob);
    let blobs = backups_root(dir.path()).join(BLOBS_DIR);
    std::fs::create_dir_all(&blobs).unwrap();
    std::fs::write(blobs.join(&sha), blob).unwrap();

    let outside = tempfile::tempdir().unwrap();
    let absolute = outside.path().join("evil.txt").to_string_lossy().into_owned();
    let cases = [
        ("../../evil.txt".to_string(), sha.clone()),
        ("ws/../../evil.txt".to_string(), sha.clone()),
        (absolute.clone(), sha.clone()),
        ("ws/ok.txt".to_string(), format!("../{}", manifest.id)),
    ];
    for (index, (storage_path, entry_sha)) in cases.into_iter().enumerate() {
        rewrite_manifest(&backup_dir, |m| {
            m.artifacts = vec![ArtifactFileEntry { storage_path: storage_path.clone(), size_bytes: 7, sha256: entry_sha }]
        });
        let target = dir.path().join(format!("restore-{index}"));
        let result = restore_backup(&backup_dir, &target, T0);
        assert!(matches!(result, Err(BackupError::InvalidBackup(..))), "{storage_path}: {result:?}");
        assert!(!target.exists(), "{storage_path}: 被拒絕時不寫入任何檔案");
    }
    assert!(!dir.path().join("evil.txt").exists() && !std::path::Path::new(&absolute).exists());
}
