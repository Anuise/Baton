VERDICT: PASS
CRITERIA:
- [PASS] 已合併整合分支 — `git merge-base --is-ancestor cli/v1 HEAD` 成立（輸出 `ANCESTOR_OK`）。`cli/v1..HEAD` 共 7 個 commit，從 ffee92c 到 7270256。
- [PASS] 等待端點：啟動時已有未確認事件就立即回傳，否則等到事件或逾時（用 fake Clock，沒有真實 sleep）— `crates/baton-api/src/controller_wait.rs:56-87`：每輪先訂閱提交通知，再查游標之後的事件。查到事件就立即回傳；到期限則回 `timed_out:true`；其餘情況用 `select!` 等三者之一：寫入提交、`clock.sleep_until(deadline)`、Runtime 停止。fake Clock 的實作在 `crates/baton-harness/src/fakes.rs`，只有測試推進時間（`advance_millis`／`set`）時才醒，並提供 `wait_for_sleepers` 讓測試確認端點已在等待。情境測試 `an_event_that_happened_before_waiting_is_returned_at_once` 驗證「時間不推進，`timeout_seconds:600` 仍立即回傳」；`without_events_the_wait_returns_when_an_event_arrives` 驗證有事件就醒；`without_events_the_wait_times_out_by_the_clock` 推進 29 999 ms 時仍在等，再推 1 ms 才回 `timed_out`（皆在 `crates/baton-harness/tests/controller_wait.rs`）。
- [PASS] 確認後同一事件不再回傳；四種事件都會觸發 — 游標依 Controller Run 存在 `controller_event_cursor` 表，有 trigger 保證只能往前（`crates/baton-store/src/lib.rs` 的 migration）。確認規則在 `crates/baton-domain/src/controller_wait.rs` 的 `acknowledge`：只往前，負數或超過最後一筆事件都拒絕。`acknowledged_events_are_not_returned_again` 驗證四件事：未確認前再等一次會拿到同一批；確認後不再回傳；`ack:0` 不會倒退；ack 超過最後一筆事件回 400 `wait_cursor_invalid`。`each_of_the_four_kinds_of_event_ends_the_wait` 涵蓋 `run_ended`、`task_blocked`、`decision_answered`、`message_received` 四種。「收到訊息」事件在存入收件匣的同一個交易中追加（`crates/baton-store/src/messages.rs:400` 呼叫 `record_message_received`）。另有 `events_of_other_workspaces_are_not_reported`（其他 Workspace 的事件不回報）與 `a_reopened_controller_continues_from_where_the_previous_one_confirmed`（重開的 Controller 接續上一個確認到的位置）。
- [PASS] Worker 憑證不能呼叫等待端點；Controller Run 結束後憑證被拒 — 端點先執行 `require_controller`，Worker 憑證回 403 `controller_only`，而且在解析 body 之前就檢查。每次被叫醒都重新執行 `controller_scope`，Run 已結束時回 401 `credential_revoked`（`crates/baton-api/src/controller.rs:61-62`）。`a_worker_credential_cannot_wait` 驗證 Worker 憑證在 body 合法與不合法時都得到 403。`ending_the_controller_refuses_its_credential_even_while_waiting` 驗證：正在等的請求在宿主回報結束後得到 401，之後的新請求也是 401。
- [PASS] `wait` 子命令：有事件以 0 結束並印摘要；逾時與憑證失效各有固定且不同的結束碼；憑證不出現在參數或輸出 — 結束碼定義在 `crates/baton-domain/src/controller_wait.rs`：0 有事件、1 其他失敗、2 用法錯誤、3 憑證失效、4 逾時。`crates/baton-mcp-shim/src/wait.rs` 的流程是：憑證只從環境變數讀、只放在 `bearer_auth`；先把摘要寫出並 flush，再送 ack。`crates/baton-mcp-shim/tests/wait.rs` 有 10 個測試，涵蓋程序內呼叫與真實執行檔，並斷言 stdout 和 stderr 都不含憑證 `TOKEN`。宿主會以 `BATON_MCP_SHIM` 環境變數把 shim 路徑交給 claude，由 `passes_the_shim_path_to_the_child` 驗證。
- [PASS] 以 Runtime harness 撰寫的情境測試涵蓋端點並通過；shim 子命令沿用既有測試方式 — 執行結果：`tests\controller_wait.rs` 10 passed，`baton-mcp-shim tests\wait.rs` 10 passed，Domain 的 `controller_wait::tests` 10 筆全部 ok。
- [PASS] AI 決策已追加至 ai-decisions.md — `.scratch/desktop-controller-cli/ai-decisions.md` 只新增了 AI-23～AI-28，每條都有重要度，沒有改動既有條目（diff 中只有新增的行）。
TESTS: `node scripts/test.mjs`（即 `pnpm test`）結束碼 0，結果是 Clippy 通過、Rust 通過、前端通過、UI E2E 通過。`node scripts/build.mjs`（即 `pnpm build`）結束碼 0，release 建置 3 分 03 秒，產出 `target\release\baton-desktop.exe`。事前執行了 `pnpm install --frozen-lockfile`（只產生被忽略的 node_modules）。驗證後 `git status --short` 是乾淨的。
FIXES_NEEDED: none

以下兩點不影響這次判定，可在後續票處理：
1. 依 AI-25，如果上一個 Controller Run 從沒呼叫過 wait（沒有游標紀錄），新開的 Run 會從自己的開啟事件開始。這段期間（上一個 Controller 開著卻沒等）發生的事件就不會回報。這屬於已記錄的取捨。
2. 操作手冊（票 04）需要寫明結束碼 0～4 與 `"$BATON_MCP_SHIM" wait` 的用法（AI-26、AI-27）。
exit: 0

