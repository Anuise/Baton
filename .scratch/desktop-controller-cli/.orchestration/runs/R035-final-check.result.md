所有檢查都通過，沒有需要修正的項目。合併後的 `cli/v1`（ae00de7）完整跑過一次，結果全綠。

```
VERDICT: PASS
CHECKED:
- [PASS] 工作樹與版本 — HEAD ae00de7；cli/v1（c27c70a）只多了 runs.md 與 R035 prompt 兩個 scratch 檔；測試與建置前後 `git status --short` 都沒有輸出，只有 node_modules、target、web/dist、src-tauri/gen 這些忽略檔
- [PASS] Controller CLI 啟動規格 — crates/baton-runtime/src/claude_adapter.rs:55 `inherited_to_clear` 清除 CLAUDE*／ANTHROPIC*／AI_AGENT／BAGGAGE；:172-176 `base_spec` 設定 `CLAUDE_CONFIG_DIR`；:108 `--strict-mcp-config --mcp-config`；:127 加上 `--append-system-prompt-file`，路徑由 :211 `controller_spawn_spec` 指向 Run 唯讀目錄裡的操作手冊；`--continue` 只在 `continue_session` 為真時加一次；`-p`／`--output-format`／`--permission-prompt-tool` 只出現在 Worker 路徑。`continue_session` 在 supervisor/controller_cli.rs:62、:368 依原生 Session 是否存在決定（domain controller_cli.rs:389）。對應測試：claude_adapter.rs:701（沒有 `-p`、權限模式、`--dangerously-skip-permissions`；env_remove 與 Worker 相同；CLAUDE_CONFIG_DIR；操作手冊檔只出現 1 次）、:785（`--continue` 剛好 1 個；Worker 沒有）、:741（沒有原生 Session 檔就不接續）；harness controller_cli.rs:126、controller_cli_accounts.rs:243（同帳號且 Session 存在才帶 `--continue`；換帳號或換 Workspace 不帶）、controller_playbook.rs:112（操作手冊唯讀、有版本號）
- [PASS] Controller 憑證失效 — baton-api/src/controller.rs:51-62 `controller_scope` 經 Store controller_cli.rs:296 呼叫 domain controller_cli.rs:476 `credential_valid`（未結束而且 `rebound_at` 為空）。測試：Run 結束 controller_cli.rs:219/244/266；重新綁定 controller_cli_exit_and_recovery.rs:268（`rebound_at` 有值、舊憑證 401、只綁定一次）；換帳號 controller_cli_accounts.rs:307（舊憑證 401）；Worker 憑證等待被拒 controller_wait.rs:346（403 controller_only）；等待途中結束 controller_wait.rs:360（401）
- [PASS] Controller 帳號排除 — 判斷在 domain scheduler.rs:403（`controller_account`）。一般派工與佇列重試都用 baton-store/src/scheduling.rs:97/107/139；交接承接用 handoffs.rs:613-621；帳號清單來自 store controller_cli.rs:316（starting／running 的 cli Run）。測試：harness controller_cli_accounts.rs:181（排隊原因是 controller_account，既有 Worker 不被停，結束後佇列派給它）、:223（改派其他帳號）；domain scheduler/tests.rs:393（Normal、preferring、Handoff 三種位置都排除）
- [PASS] 完全退出、崩潰恢復、當掉後換帳號 — supervisor.rs:411-431 把 Controller CLI 列入停止進度並與 Worker 一起停止；controller_cli.rs:161-202 處理重啟後的三種情況：宿主存活就 rebind、沒有回執就結束、宿主不在就 interrupted 並在同一交易內帶入換帳號要求；web/src/ExitProgress.tsx:140 顯示。測試：controller_cli_exit_and_recovery.rs:160（stop_calls、full_exit、exit.runs 的 controller_cli=true）、:196、:214、:247（interrupted、不重開窗）、:268（rebound）；controller_cli_accounts.rs:486（當掉後仍以新帳號開新 Run、第 2 個視窗）
- [PASS] 舊對話移除 — web/src/App.tsx:60-70 只有 9 個 Workspace 頁簽，沒有對話；App.test.tsx:431 測試記住的舊「conversation」頁簽會回到 Controller；runtime supervisor.rs:34 註明自動輪次已移除，正式程式碼已沒有 ConversationNotice、WorkflowNote、enqueue_work_events；資料表 migration 保留在 baton-store/src/lib.rs:1112/1145。測試：retired_conversation.rs:130（端點 404）、:146（工作事件不產生 ctl_ 輪次）、:195（升級前資料重啟後原樣保留）；store lib.rs:2876
- [PASS] `.baton/ai-decisions.md` — supervisor/controller_cli.rs:378 `ensure_ai_decisions` 在開窗前呼叫 baton-ports/src/workspace_fs.rs:149 `create_file_if_absent`，以 create_new 建立，不覆寫。測試：controller_playbook.rs:161（以範本建立）、:174（已存在不覆寫）、:190（建立失敗就不開並說明原因）；domain playbook/tests.rs:82
- [PASS] ai-decisions.md 編號 — 以 python 解析：63 個 `### AI-NN` 條目，1～63 連續、沒有重複，每條都有 `- 重要度：高/中/低` 和 `- 審閱：` 行（共 64 行審閱，多的 1 行是檔頭的格式範本）
- [PASS] 遺留物 — 沒有 headless Chromium、driver.mjs 或 ui_e2e 程序（driver.mjs 只以追蹤中的原始碼存在）；%TEMP% 沒有 baton-e2e-*；%TEMP%\baton-harness 是空的；正在跑的 node、baton-desktop/runtime（使用者已安裝的 App）都在本 run 開始（20:44）之前就已啟動，不是這次測試留下的
TESTS:
- pnpm test 照 scripts/test.mjs 拆成四段，全部在前景跑完：
  - Clippy `cargo clippy --workspace --all-targets -- -D warnings`：通過（結束碼 0）
  - Rust `cargo test --workspace --exclude baton-desktop`：通過，135 個 test result，1874 個通過、0 失敗、14 個 ignored，耗時約 6 分 22 秒
  - 前端 `pnpm --filter web test`：30 個檔案、678 個測試全過
  - UI E2E（BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw，`node scripts/test-e2e.mjs`）：4 個通過，包含 controller_cli_open_and_end；跑完沒有遺留
- `pnpm --filter web typecheck`：通過
- `cargo check -p baton-desktop`：通過
- `pnpm build`：通過，release 版 baton-runtime、baton-guardian、baton-desktop.exe 都建置成功
FIXES_NEEDED: none
```

以下是不影響判定的低優先觀察：
- review-notes 第 13、14 項依指示仍未處理。這次 baton-client 的 shared_settings_client 測試沒有失敗。
- 交接承接的排除只有 domain 單元測試（scheduler/tests.rs:400），沒有 harness 情境測試。
- crates/baton-domain/src/decision/tests.rs:569 的測試註解還留著「喚醒 Controller」的舊用語。
exit: 0

