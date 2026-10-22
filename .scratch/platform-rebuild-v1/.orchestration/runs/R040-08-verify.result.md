```
VERDICT: PASS
CRITERIA:
- [PASS] 每次採用建立新版本，舊版本內容不被修改 — Domain 的 `adopt` 每次都取「最新版本號 + 1」（crates/baton-domain/src/shared_settings.rs:487-511）。Store 在同一個交易裡讀 MAX(version)、INSERT 新版本並追加事件（crates/baton-store/src/shared_settings.rs:12-34）。migration 對 shared_settings_version 與 run_settings_snapshot 都加了禁止 UPDATE/DELETE 的觸發器（crates/baton-store/src/lib.rs 新增的 migration）。驗證這條的測試：store 單元測試 `adopted_versions_cannot_be_modified_or_deleted_even_with_raw_sql`（用原始 SQL 改寫或刪除，兩者都失敗）、harness 情境測試 `each_adoption_creates_a_new_version_and_earlier_versions_never_change`（v1 → v2 → v3，採用新版本後讀回的 v1 與當初完全相同；內容沒變時仍建立 v3）、`adopted_versions_survive_a_runtime_restart`。三者皆 ok。
- [PASS] 匯入結果不含任何登入秘密（以測試掃描驗證） — 來源白名單：`DirUserSettingsSource` 只讀 settings.json、CLAUDE.md、.claude.json 三個檔案（crates/baton-ports/src/user_settings.rs:36-44），.claude.json 只取 mcpServers（shared_settings.rs:200-205）。另外有登入相關鍵清單、login env 排除，以及依名稱與內容的遮蔽（shared_settings/secrets.rs）。情境測試 `imported_content_never_contains_login_secrets_anywhere` 放入 11 種秘密：.credentials.json 的 OAuth token、ANTHROPIC_API_KEY、apiKeyHelper、oauthAccount 的 email／uuid、GITHUB_TOKEN、Bearer、hook 內的 x-api-key、CLAUDE.md 內的 ghp_、以金鑰命名的 MCP 伺服器。測試逐位元組掃描預覽、採用、讀回、列表與快照的回應、快照目錄、control.sqlite（含 -wal/-shm）以及日誌，結果 ok。我另外寫了外部探測程式（放在 repo 外、已刪除）多試 10 種登入秘密形狀，泄漏都是 false：statusLine 指令內的 CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat…、MCP env 的 OAuth token、MCP args 裡的 sk-ant-oat、以金鑰為名的偏好鍵、小寫 anthropic_auth_token、CLAUDE.md 內的 sk-ant-oat、permissions 內的 Bearer、URL 帳密、URL query 的 apiKey、.claude.json 的 primaryApiKey/userID。
- [PASS] 可為指定 Run 產生唯讀的設定快照目錄，內容與版本雜湊一致 — 相關程式：`materialize_run_settings`（crates/baton-api/src/shared_settings.rs:180-221）、`write_readonly_tree` 先寫暫存目錄、設唯讀屬性再改名（crates/baton-ports/src/workspace_fs.rs）、`snapshot_files` 產生的 manifest 記錄每個檔案的 sha256（shared_settings.rs:535-566）。情境測試 `run_settings_snapshot_is_a_read_only_directory_matching_the_version_hash` 驗證：用 `bundle_from_snapshot_files` 從目錄還原內容後，`content_hash` 等於版本雜湊、manifest 雜湊與實際檔案相符、每個檔案都無法開啟寫入或覆寫。`a_run_stays_pinned_to_its_version_while_later_runs_use_newer_ones` 驗證已綁定的 Run 不受新版本影響、要求改用別的版本會回 409。另外 `a_run_snapshot_and_its_version_binding_survive_a_runtime_restart` 也是 ok。
- [PASS] 使用者原始設定檔在匯入前後位元組相同 — Port 只呼叫 `std::fs::read`，沒有任何寫入能力。情境測試 `the_users_original_settings_are_byte_identical_before_and_after_import` 比對整個假家目錄（含 .credentials.json）的檔案清單與位元組，經過預覽、兩次採用、產生快照後完全相同，結果 ok。
- [PASS] 以主要 seam（Runtime harness，D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加至 ai-decisions.md — crates/baton-harness/tests/shared_settings.rs 共 13 個測試，全部經真實 HTTP API 操作，13 passed。ai-decisions.md 新增 AI-103～AI-112，每條都標了重要度。編號接在 rebuild/v1 的最後一條 AI-102 之後，沒有重複編號。
- [PASS] 前置：rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 輸出 ANCESTOR_OK（rebuild/v1 = bff39c3）。
TESTS:
- `pnpm test`：TEST_EXIT=0。Rust 47 個測試執行檔全部 `test result: ok`、0 failed，其中 harness shared_settings 13/13、domain shared_settings 單元測試全過、baton-client shared_settings_client 通過。前端 vitest 12 個測試檔、169 tests 全過。
- `pnpm build`：BUILD_EXIT=0，產生 target/release/baton-desktop.exe（含 web/dist 與 runtime）。
- 驗證後 `git status --short` 為 0 行，工作樹乾淨，沒有修改任何追蹤中的檔案。
FIXES_NEEDED: none
（以下觀察不影響這次判定，留給後續票參考：
 1. `materialize_run_settings` 遇到同一個 Run 同時有兩個請求、而且要求不同版本時，兩邊會共用同一個 `<run_id>.staging` 暫存目錄與同一個目標目錄。後到的請求可能先把目錄短暫寫成另一個版本才改回，也可能直接回 500。目前呼叫端只有桌面 UI 與單一准入流程，實際風險低。後續 Adapter 票接上准入時，建議改成依 Run 序列化，或讓暫存目錄名稱各自唯一。
 2. 已經綁定的 Run 再次要求快照時，直接回傳綁定資訊，不檢查目錄是否還在、內容是否被竄改。這點 AI-107 已記錄，但 Adapter 啟動前應依 manifest.json 核對。
 3. 秘密偵測是啟發式的：不帶已知前綴、也沒有秘密欄位名稱的值仍會被保留，例如 MCP args 裡的 `["--api-key","plainvalue…"]`。這類是第三方服務的金鑰，不是 Claude 登入秘密，AI-104 也已記載這個限制。
 4. `materialize_run_settings` 目前是 `pub(crate)`，還沒接到派工或 Supervisor 流程。這在票面「提供能力」的範圍內，AI-106 也已記錄。）
```
