票 08 驗證不通過。建置與全部測試都過了，但發現一個缺陷：當秘密出現在 JSON 鍵名時，會被原樣寫進排除紀錄，違反第 2 條驗收條件。

```
VERDICT: FAIL
CRITERIA:
- [PASS] rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 輸出 ANCESTOR_OK（rebuild/v1 = 84633bc）
- [PASS] 每次採用建立新版本，舊版本內容不被修改 — Domain `adopt` 版本號 = 最新 + 1（crates/baton-domain/src/shared_settings.rs `adopt`）；Store 在 `shared_settings_version` 表加了禁止 UPDATE/DELETE 的觸發器（crates/baton-store/src/lib.rs migration「shared_settings_version」）；測試 `each_adoption_creates_a_new_version_and_earlier_versions_never_change`、`adopted_versions_cannot_be_modified_or_deleted_even_with_raw_sql`、`adopted_versions_survive_a_runtime_restart` 通過
- [FAIL] 匯入結果不含任何登入秘密（以測試掃描驗證） — 情境測試 `imported_content_never_contains_login_secrets_anywhere` 通過，掃描範圍涵蓋回應、快照目錄、SQLite（含 WAL）與日誌。但缺陷已實測重現：crates/baton-domain/src/shared_settings.rs `import_item` 判斷名稱本身像秘密（`secrets::contains_secret_value(&name)`）後，仍把含名稱的 `path` 原樣寫進 `Exclusion.item`。我在 repo 外建了一個暫時 crate 呼叫 `import`，輸出如下：
    · settings.json `{"ghp_AAAA…":true}` → exclusions `[{"item":"ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAA","reason":"secret_value"}]`
    · .claude.json `{"mcpServers":{"sk-ant-api03-ZZZZ…":{…}}}` → `{"item":"mcpServers.sk-ant-api03-ZZZZZZZZZZZZZZZZ"}`
  排除清單屬於 ImportCandidate／SettingsVersion，會存進資料庫 `exclusions` 欄位、在預覽 API 回傳並顯示在 UI。這違反本條驗收條件，也違反 AI-96 與程式註解「排除紀錄絕不含值」的保證。
- [PASS] 可為指定 Run 產生唯讀的設定快照目錄，內容與版本雜湊一致 — 端點 `POST /api/v1/workspaces/{id}/runs/{run_id}/settings-snapshot` 與 `materialize_run_settings`（crates/baton-api/src/shared_settings.rs）；`write_readonly_tree`（crates/baton-ports/src/workspace_fs.rs）先寫暫存目錄、設唯讀再改名；測試 `run_settings_snapshot_is_a_read_only_directory_matching_the_version_hash` 用 `bundle_from_snapshot_files` 還原內容，重算的雜湊等於版本雜湊、manifest 各檔 SHA-256 一致、檔案無法開啟寫入；`a_run_stays_pinned_…`、`a_run_snapshot_and_its_version_binding_survive_a_runtime_restart` 通過。唯讀屬性不防刪除，已記為 AI-99。
- [PASS] 使用者原始設定檔在匯入前後位元組相同 — `UserSettingsSource` Port 只有 `read`（crates/baton-ports/src/user_settings.rs），沒有寫入路徑；`FakeUserSettings::files()` 比對整個家目錄（含 `.credentials.json`）；測試 `the_users_original_settings_are_byte_identical_before_and_after_import` 通過
- [PASS] 以主要 seam（Runtime harness）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加 — crates/baton-harness/tests/shared_settings.rs 的 13 個情境測試全部 ok（測試紀錄第 433–445 行）；ai-decisions.md 新增 AI-95～AI-102。小瑕疵：這些條目插在 AI-94 之前，不在檔尾，且 AI-102 與 AI-94 標題之間缺空行，不影響判定。
TESTS:
- `pnpm build` → BUILD_EXIT=0（release 版 baton-runtime 與 Tauri 桌面 App 都建置成功）
- `pnpm test` → TEST_EXIT=0；Rust：通過（所有 `test result: ok`，0 failed，另有 3 ignored，屬於本票以外的既有測試）；前端：通過（Test Files 11 passed，Tests 146 passed）
- 執行後 `git status --short` 無輸出，沒有改動任何追蹤中的檔案
FIXES_NEEDED:
1. crates/baton-domain/src/shared_settings.rs `import_item`：`secrets::contains_secret_value(&name)` 成立時，`Exclusion.item` 不可含原名稱，改成遮蔽後的路徑（例如 `mcpServers.<已遮蔽>`，頂層偏好鍵則只寫 `<已遮蔽>`）。順便確認 `clean` 與 `without_login_env` 寫入的子路徑，不會用到被判定為秘密的鍵名。
2. 在 crates/baton-domain/src/shared_settings/tests.rs 補單元測試：秘密出現在頂層偏好鍵、MCP 伺服器名稱與 hook 事件名稱時，`serde_json::to_string(&candidate)` 不含該秘密。
3. 在 crates/baton-harness/tests/shared_settings.rs 的 `plant_settings_with_secrets` 加一個以秘密為名稱的 MCP 伺服器，並把它放進 `PLANTED_SECRETS`，讓情境掃描涵蓋這條路徑。
4. 非必要：AI-96 已承認啟發式偵測的限制，這裡補一個實測案例。hook 指令 `curl -H 'x-api-key: abcdefghijklmnop1234567890'`（沒有已知前綴）目前會被原樣匯入。可考慮把 `名稱: 值` 的標頭形式（例如 `x-api-key:`、`authorization:`）納入偵測，或至少在 AI-96 註明。
```
