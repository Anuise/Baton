```
VERDICT: PASS
CRITERIA:
- [PASS] 管理 API 可用既有目錄與名稱建立 Profile；建立後立即核對登入身分，結果顯示在帳號畫面 — 端點是 `POST /api/v1/profiles/from-existing-dir`（crates/baton-api/src/lib.rs:297、crates/baton-api/src/profiles.rs:123-174）。登記成功後會呼叫 `verify_profile`（profiles.rs:172-173），回傳的 view 帶核對結果。client 方法 baton-client/src/lib.rs:180，Tauri 指令 `profile_create_from_dir` 在 src-tauri/src/main.rs:245 並已註冊（:1372）。帳號畫面表單在 web/src/AccountsPage.tsx:266、273-330，外部目錄標示在 :199-201。情境測試 `an_existing_logged_in_dir_becomes_a_dispatchable_external_profile_without_a_new_login` 斷言核對次數加 1、account／expected_account 已固定、dispatchable=true、沒開登入終端機、GET 清單與回應一致。另有未登入、身分重複、核對失敗仍建立（AI-08）三個情境，都通過。
- [PASS] 目錄不存在、與其他 Profile 相同或互相包含、位於平台資料目錄的 profiles 底下時被拒，並顯示原因 — 目錄不存在回 `profile_config_dir_not_found`，路徑是檔案回 `profile_config_dir_not_directory`，兩者都在 profiles.rs:146-165，透過 `inspect_directory` 檢查，平台不會替使用者建目錄。相同或互相包含的判斷在 domain/profile.rs:382-385、502-504，不分大小寫，斜線方向、`\\?\` 前綴與結尾分隔線都正規化，已封存的 Profile 也算在內，錯誤訊息是「相同或互相包含」。落在平台 profiles 底下或包含它，回 `profile_config_dir_managed`（profile.rs:410-412，managed_root 等於 `data_dir/profiles`，profiles.rs:139）。非絕對路徑回 `profile_config_dir_invalid`。情境測試 `a_missing_dir_or_a_file_is_rejected...`、`a_dir_that_is_the_same_as_or_nested...`（含大寫、子目錄、上層目錄）、`a_dir_under_the_platforms_profiles_dir_is_rejected` 都通過；UI 測試「Runtime 拒絕時在表單上顯示它的說明並保留輸入」也通過。
- [PASS] 封存外部目錄的 Profile 不刪除、不修改該目錄；平台建立的 Profile 行為不變 — `archive` 只套用 Domain 命令，不碰檔案系統（profiles.rs:215-221），正式程式碼裡沒有對設定目錄做 remove 或 write。我只找到一處讀 `projects/` 的唯讀檢查（claude_adapter.rs:248）。情境測試 `archiving_an_external_profile_neither_deletes_nor_modifies_its_dir` 比對建立前與封存後整棵目錄的快照，`archiving_a_platform_created_profile_behaves_as_before` 確認平台建立的 Profile 標為 managed，目錄快照也不變。migration 給既有資料預設 `managed`，並加了 CHECK 限制（baton-store/src/lib.rs:1839-1845）。
- [PASS] 以既有目錄建立的 Profile 可正常被派工（以 harness 驗證 Worker Run 使用該目錄作為 `CLAUDE_CONFIG_DIR`） — 情境測試 `a_worker_run_on_an_external_profile_uses_that_dir_as_its_claude_config_dir` 搭配正式的 `ClaudeCodeAdapter` 派工，斷言啟動規格的 env 含 `CLAUDE_CONFIG_DIR=<外部目錄>`，且 settings.json 沒被改。結果 ok。
- [PASS] 以 Runtime harness 撰寫情境測試涵蓋上述行為並通過；帳號畫面的元件測試涵蓋新表單 — crates/baton-runtime/tests/profiles_from_existing_dir.rs 共 12 個測試，全部 ok。web/src/AccountsPage.test.tsx 新增 4 個元件測試：成功送出、被拒時顯示原因並保留輸入、空欄位不送出、8 個上限時停用。另有 runtimeClient.test.ts 的 HTTP／Tauri 對應測試、baton-client/tests/profiles_client.rs 的 client 測試，以及 domain 單元測試 7 個。
- [PASS] AI 決策已追加至 ai-decisions.md — .scratch/desktop-controller-cli/ai-decisions.md 追加了 AI-06～AI-09，各附重要度（中／低／低／低），格式符合，只追加、沒改舊條目。
TESTS:
- `git merge-base --is-ancestor cli/v1 HEAD` → ANCESTOR_OK（cli/v1 是 HEAD 的祖先）
- `pnpm test`（Clippy -D warnings、cargo test --workspace --exclude baton-desktop、web vitest、UI E2E）→ EXIT=0。Clippy、Rust、前端、UI E2E 四項都通過。Rust 合計 1801 passed、0 failed、12 ignored（被忽略的是 UI E2E 測試，由 test.mjs 另外執行，結果 3 passed）。前端 30 個測試檔、655 個測試全過。
- `pnpm build`（cargo build --release runtime/guardian 加 tauri build --no-bundle）→ EXIT=0，產出 target/release/baton-desktop.exe
- 跑完後 `git status --short` 是空的，沒有改動任何追蹤中的檔案。
FIXES_NEEDED: none
```

有三點不擋這張票，但值得知道：

1. 依 AI-06 的決定，封存後的外部目錄不能再登記成新的 Profile，目前也沒有「取消封存」可以把它加回來。
2. 立即核對失敗時，回應不會帶失敗原因（AI-08），使用者要按「重新核對」才看得到。
3. `AccountsPage.test.tsx` 與 `runtimeClient.test.ts` 的假錯誤用了程式碼裡不存在的錯誤碼（`profile_dir_overlaps`、`profile_dir_missing`）。測試只斷言 message，所以不影響結果。
exit: 0

