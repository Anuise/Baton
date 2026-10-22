VERDICT: PASS
CRITERIA:
- [PASS] 前提：`rebuild/v1` 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 成功（rebuild/v1 = 7dfbb4d，HEAD = 454339f）。
- [PASS] 還原演練在乾淨目錄成功，資料與成果雜湊一致 — 備份用的是 SQLite Online Backup API（`crates/baton-store/src/backup.rs:168-175`）。`restore_backup` 只接受空目錄或不存在的目錄（`:323-325`），還原時核對五項：快照雜湊、`integrity_check`、`foreign_key_check`、成果檔案的大小與雜湊、資料庫登記的成果版本與 manifest 一致（`:330-375`，`crates/baton-domain/src/backup.rs:205-238`）。
  - 情境測試 `restore_drill_in_a_clean_directory_matches_the_data_and_artifact_hashes`（`crates/baton-harness/tests/backup_and_restore.rs:162-206`）在還原出來的新 Runtime 上，逐一下載每個成果版本，內容與 SHA-256 都和原本相同。
  - 情境測試 `a_damaged_artifact_fails_the_drill_and_the_settings_page_shows_it`（`:208-231`）確認成果損毀時演練判定不通過。
  - 我另外用 release 版 `baton-runtime.exe` 實跑：開一個暫存資料目錄啟動，Runtime 自動建立每日備份，`drill.json` 五項都通過。`baton-runtime restore <備份> <空目錄>` 結束碼 0；對同一個非空目錄再跑一次，被拒絕，結束碼 1。
  - 結果會顯示在設定頁：`GET /api/v1/backups`（`crates/baton-api/src/backups.rs`）、`web/src/App.tsx:62` 的 `BackupSettings`、Tauri 指令 `backup_status` 都有接上。
- [PASS] 備份內容不含任何憑證或原生登入資料（掃描測試） — 備份只含資料庫快照與成果，快照裡的 `credential` 表會清空再 VACUUM，並改用 DELETE journal，不會另外留下 `-wal` 檔（`backup.rs:182-191`）。
  - 掃描測試 `backups_contain_no_credentials_or_native_login_data`（`backup_and_restore.rs:255-304`）逐位元組掃整個 `backups/`。掃描對象包括：原生登入 token、`.credentials.json` 的內容、桌面憑證與 Agent 憑證的原文和雜湊、桌面憑證檔。結果都不存在，備份路徑中也沒有 `profiles/`、`auth/`。
  - 實機核對：正式資料庫的 `credential` 有 1 列，備份裡是 0 列，正式資料庫的 token_hash 也不出現在備份檔的位元組中。
  - 共同設定在先前的票就已在存入前遮蔽秘密（`shared_settings.rs` 的 `MASKED`）。
- [PASS] 還原後外部操作不被自動重放 — 還原時 `end_unfinished_runs`（`backup.rs:387-406`）把已啟動的 Run 標為 `interrupted`（原因 `restored_from_backup…`），queued 的 Run 取消，Task 回到 ready。
  - 情境測試 `after_restore_unfinished_runs_are_not_relaunched_or_resumed`（`backup_and_restore.rs:308-345`）：還原後新 Runtime 跑了三次 supervise，`spawn_count==0`、`launched()` 為空、`inspect_count==0`，舊的 Agent 憑證得到 401，原本的 Runtime 不受影響。
  - `decision_wake:` 類 Outbox 沒有任何消費端，不構成重放風險。
- [PASS] 以 Runtime harness（spec D-04）寫的情境測試涵蓋本票行為並通過，AI 決策已追加 — `backup_and_restore.rs` 共 9 個情境測試全部通過，涵蓋演練、損毀偵測、每週演練、憑證掃描、不重放、每日與重要裁定觸發（每小時最多一次）、正式裁定觸發備份、7 份每日加 4 份每週的保留。`ai-decisions.md` 已追加 AI-168～AI-176，每條都標了重要度。
- [PASS] 票面其他要求 — 三種觸發都有實作：
  - migration 前：備份失敗就不套用 migration（`crates/baton-store/src/lib.rs` 的 `migrate`，`StoreError::PreMigrationBackup`，store 測試 `failed_pre_migration_backup_leaves_the_database_unmigrated` 通過）。
  - 每日。
  - 重要裁定：接在 `crates/baton-api/src/decisions.rs:102`。
  - 「備份期間暫停成果清理」：目前程式裡沒有任何會刪成果的機制，AI-175 已記錄這點，也記下日後加清理時必須和 `BackupScheduler::tick` 互斥。我接受這個判斷。
TESTS:
- `pnpm build` → 成功（`Finished release … 2m 04s`，產出 `target/release/baton-desktop.exe`、`baton-runtime.exe`）
- `pnpm test` → 結束碼 0。Rust：`cargo test --workspace --exclude baton-desktop` 共 834 個通過、0 失敗、3 個 ignored，其中包含 `backup_and_restore.rs` 的 9 個，以及 store、domain 的 backup 單元測試。前端：Vitest 16 個檔案、284 個測試通過。
- release 執行檔的手動冒煙（暫存目錄，跑完已刪除）：自動備份與演練通過、`restore` 到空目錄成功、到非空目錄被拒、備份中憑證 0 列。
- 工作樹仍然乾淨（`git status --short` 沒有輸出）。
FIXES_NEEDED: none

不影響判定的小問題，供參考：
1. `baton-runtime restore` 失敗時，錯誤訊息前面會帶著「Runtime 啟動失敗：」（`main.rs` 共用了這個錯誤前綴），用在還原指令上會誤導人。
2. `restore_backup` 直接把 manifest 裡的 `storage_path` 接到目標路徑上，沒有防 `..` 路徑穿越。manifest 是 Baton 自己產生的，風險低。
3. spec D-18 的「使用者另外指定的額外備份目錄」（spec 寫明非必要）沒有實作，AI-175 和開發文件的已知限制都有寫。
