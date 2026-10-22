VERDICT: PASS
CRITERIA:
- [PASS] Controller 憑證可以呼叫 `search_knowledge`、`send_message`、`read_inbox` 與 Controller 版的 `record_lesson_candidate`，範圍限於自己的 Workspace。證據：
  - `crates/baton-api/src/guard.rs:33-39` 的 `CONTROLLER_SHARED_TOOLS` 放行 5 個工具。
  - `messages.rs` 的 `actor_of` 對 Controller 一律走 `controller_scope`：這一輪必須仍在進行，憑證與請求帶的 `workspace_id` 不一致時回 403。
  - store 的 `sender_of` 以 `controller_run WHERE id AND workspace_id` 查詢；`read_inbox` 讀 `message_controller_delivery WHERE workspace_id=?1`。
  - `record_controller_lesson_candidate` 在交易中執行 `SELECT goal_id FROM task WHERE id=?1 AND workspace_id=?2`，查不到就回 `task_not_found`，不寫入。
  - 情境測試全數 ok：`controller_messages.rs` 3 個、`controller_lesson_candidate.rs` 4 個、`controller_shared_tools.rs` 1 個。
- [PASS] Controller 可依 spec 接受成果，仍受驗證 Gate 約束。證據：
  - 實作在 `crates/baton-api/src/verification.rs` 新增的 `accept_artifact`，並已加入 `CONTROLLER_TOOLS`（`agent.rs`）。
  - 只在自己的 Workspace 內查 Task。Task 不是 verifying 時回 409 `task_not_verifying`。契約宣告了確定性檢查時回 409 `verification_gate_pending`，不建立裁定請求。
  - 沒有宣告檢查時以 `skip_stage_or_gate` 送裁定。這個事項內建必須送裁定，Workspace 不能放寬（`authority.rs:293`）。裁定綁定 Run、世代、契約版本與成果雜湊；核准後走 `AcceptManually`，`accepted_by` 記為 `decision:<id>`。
  - `controller_accept_artifact.rs` 5 個測試全部 ok，其中包含「核准只涵蓋使用者看到的那一份成果」。
- [PASS] Controller 不能讀寫其他 Workspace 的資料，Worker 的既有範圍沒有放寬。證據：
  - Controller 端（`controller_messages.rs:194-238`）：
    - 寄到別的 Workspace 的 Task 回 404 `recipient_not_found`。
    - 請求聲稱別的 `workspace_id` 回 403 `agent_scope_mismatch`。
    - 別的 Workspace 的 Worker 寄給 `controller` 的訊息，不會出現在本 Workspace 的收件匣。
  - Worker 端：
    - 讀不到 Controller 的收件匣。
    - 不能回覆 Controller 與其他 Task 之間的訊息（404 `reply_target_not_found`）。
    - 送 `task_id` 想把教訓掛到別的 Task，會被忽略，來源與範圍仍是自己的 Task（`controller_lesson_candidate.rs:258`）。
    - Worker 呼叫 `accept_artifact` 回 403 `controller_only`。
  - 其他工具：教訓與成果接受跨 Workspace 都回 404 `task_not_found`，且什麼都不寫；這一輪結束後一律回 401 `credential_revoked`。
  - Worker 的交付仍存在 `message_delivery`，以收件 Task 為界；Controller 的交付另存一表。Task 識別一律由 Runtime 以 `new_id("task")` 產生，不會與保留地址 `controller` 衝突。
- [PASS] 以 Runtime harness 撰寫情境測試並通過；MCP shim 的工具清單與說明已同步。證據：
  - 新增 3 個 harness 測試檔，並修改 `controller_shared_tools.rs`。
  - `crates/baton-mcp-shim/src/lib.rs` 新增 `accept_artifact` 的定義與轉送，並更新 `send_message`、`read_inbox`、`record_lesson_candidate` 的說明與 `task_id` 欄位。
  - shim 測試已更新為 39 個工具並通過；`docs/development.md` 新增〈Controller 的共同工具（票 54）〉一節。
  - migration 47 有搬移測試（`the_controller_messages_migration_keeps_worker_messages_and_allows_exactly_one_kind_of_sender`），結果 ok。
- [PASS] AI 決策已追加至 `ai-decisions.md`：AI-503（訊息）、AI-504（`accept_artifact`）、AI-505（教訓）、AI-506（改號），各自標了重要度。小瑕疵：AI-505 少了其他條目都有的「審閱：未審」一行，不影響判定。

TESTS:
- `pnpm build`：exit 0，產出 `target/release/baton-desktop.exe`，沒有 warning 或 error。
- `pnpm test`：exit 0。
  - Rust（`cargo test --workspace --exclude baton-desktop`）共 126 個 test suite：1774 通過、0 失敗、9 ignored。
  - 前端：649 通過。
- 前置條件：驗證開始時 `rebuild/v1`（db65671）是 HEAD 的祖先。驗證途中整合分支前進到 34dd6fd，這個 commit 只改 `.scratch/.../.orchestration`（runs.md 與 R166 的指示檔），所以目前已不是祖先。依指示只標註，不因此判 FAIL。
- 工作樹保持乾淨，沒有修改任何追蹤中的檔案。

FIXES_NEEDED: none（選擇性修正：在 AI-505 補上「審閱：未審」一行）
