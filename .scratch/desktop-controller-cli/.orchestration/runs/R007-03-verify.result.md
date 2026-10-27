```
VERDICT: PASS
CRITERIA:
- [PASS] Controller 頁簽只能選可用的 Profile，按開啟後經 Claude 環境 Port 開出終端機，fake Port 記到的啟動規格正確 — UI：web/src/ControllerPage.tsx 的 `<option disabled={!profile.dispatchable}>`，`selectable` 為 false 時開啟鈕停用；元件測試 ControllerPage.test.tsx:62、79。後端：domain controller_cli.rs 的 `open()` 以 `profile_unavailable_reason` 拒絕，harness controller_cli.rs:355 對未登入的 Profile 斷言 409 `profile_unavailable`。啟動規格：harness controller_cli.rs:125 斷言程式是 shim、`args[0..2]=[controller-host, claude]`，參數裡沒有 `-p/--print/--output-format/--permission-mode/--permission-prompt-tool/--dangerously-skip-permissions`；工作目錄＝Workspace；`CLAUDE_CONFIG_DIR`＝Profile 的目錄；env_remove 含 ANTHROPIC_API_KEY/CLAUDECODE/AI_AGENT/BAGGAGE、不含 PATH；`--strict-mcp-config --mcp-config` 指向這個 Run 的 MCP 檔（內容含 baton-mcp-shim）；憑證不在參數也不在 MCP 檔。共用組裝：claude_adapter.rs 的 `shared_args`/`base_spec`/`wrap_batch` 由 `spawn_spec` 與 `controller_spawn_spec` 共用，單元測試 `the_controller_cli_shares_the_worker_assembly_but_runs_interactively_through_the_host` 斷言兩者的 env_remove 相同
- [PASS] 開啟建立互動式 Controller Run（starting → running → ended），時間線記錄開啟、結束與帳號 — open 回 202 且 status=starting，supervise 後 running（controller_cli.rs:73-87、:139）。時間線依序為 `controller_cli_opened / launch_started / running`，category=`controller`，opened 的 payload 含 profile_id（:171-176）；`controller_cli_ended` 的 payload 也含 profile_id（:220-222）。timeline.rs 新增 `Controller` 類別
- [PASS] 同一 Workspace 已有非終態 Run 時再次開啟被拒並提示；同一 Profile 可同時當兩個 Workspace 的 Controller — harness :181 斷言 409 `controller_cli_already_open`，訊息含「已有開啟中的 Controller CLI」；第二個 Workspace 用同一個 Profile 開到 running，共開 2 個視窗。DB 另有部分唯一索引 `controller_run_one_cli_per_workspace`，migration 測試 lib.rs `the_controller_cli_migration_keeps_controller_turns_and_their_references` 涵蓋
- [PASS] 宿主回報結束、宿主消失、使用者按結束，都讓 Run 進入 ended 並使憑證失效 — harness :201（host_exited，之後 get_workflows 回 401，host/started 也回 401）、:226（Liveness::Dead → host_gone，回 401）、:248（stop 了宿主的程序身分 → user_ended，回 401，再按結束回 409 `controller_cli_not_open`）。憑證有效性依 `controller_run_active`（status 為 starting/running），見 conversations.rs:221
- [PASS] 在 launch intent 與開窗回執之間崩潰後重啟，不重複開窗 — supervisor/controller_cli.rs 的 `recover_controller_cli` 以 command_id `find`。harness :281 用 hang 加 crash_restart，結果只開過 1 個視窗並補寫回執成 running；:302 是視窗已消失的情況，結束為 `launch_outcome_unknown`，同樣沒有重開
- [PASS] 開窗失敗、找不到 claude、Profile 失效時，頁簽顯示原因且不留下非終態 Run — harness :323 涵蓋 claude_not_found（沒開窗）、terminal_failed（原因含 Port 錯誤訊息，憑證回 401），之後可以再開；:355 涵蓋開啟後 Profile 被封存 → profile_unavailable 且沒開窗。UI 以「開啟失敗：<原因>」顯示（ControllerPage.test.tsx:128）
- [PASS] Controller 憑證可呼叫既有的 Controller 工具（propose_task、dispatch_task） — harness :379 `the_controller_credential_can_propose_and_dispatch_tasks`：propose_task 回 201，dispatch_task 回 2xx，Task 面板上的狀態不再是 pending
- [PASS] harness 情境測試涵蓋並通過；Controller 頁簽有元件測試；真實 Port 有 Windows 煙霧測試或手動驗收 — harness controller_cli.rs 10 個測試全過；ControllerPage.test.tsx 9 個案例；runtime tests/controller_terminal.rs 中標 ignored 的開窗測試由我實際執行，1 passed；docs/validation.md 新增手動驗收清單 7 項
- [PASS] AI 決策已追加至 ai-decisions.md — 新增 AI-14～AI-22 共 9 條，每條都有重要度（中 5 條、低 4 條）與「審閱：未審」
TESTS:
- `git merge-base --is-ancestor cli/v1 HEAD` → 成立（cli/v1 是 HEAD 的祖先）
- `pnpm test` → EXIT=0：Clippy（-D warnings）通過；Rust 全 workspace 通過，0 failed，其中 harness controller_cli 10 passed、client controller_cli_client 2 passed、domain controller_cli 單元測試 6 passed；前端 Vitest 31 個檔案、674 個測試全過；UI E2E 3 passed
- `cargo test -p baton-runtime --test controller_terminal -- --ignored` → 1 passed（真的開出主控台，find/inspect/stop 都正常）
- `pnpm build` → EXIT=0（web、release runtime、baton-desktop.exe 都建置成功）
- `git status` → 乾淨，沒有修改任何追蹤中的檔案
FIXES_NEEDED: none
```

不影響判定的觀察：
1. spec 列的 `--append-system-prompt-file`（操作手冊）與同帳號重開帶 `--continue` 都還沒做，票面已註明由後續票處理。
2. 宿主啟動 `claude` 失敗時，回報的結束代碼是 `host_exited`，頁簽上看不出「啟動失敗」。Supervisor 開窗前已先探測過 `claude`，實際影響小。
3. 正式 Port 不直接呼叫 `wt.exe`，改由 Windows 的預設終端機設定決定用哪個終端機（AI-15）。這和 spec「優先 Windows Terminal」的字面不同，需要使用者審閱這條決策。
exit: 0

