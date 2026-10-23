VERDICT: PASS
CHECKED:
- [PASS] R026 第 1 項（低）換帳號後要不要開新 Run 改由 Store 決定 — Store 的 `apply_cli` 現在只要 Run 進入終態、而且呼叫端給了 `next`，就呼叫 `open_switched`（`crates/baton-store/src/controller_cli.rs:425`）；`open_switched` 依交易內讀到的 Run 的 `switch_to_profile_id` 決定要不要開。Supervisor 所有可能讓 Run 結束的寫入（結束、`begin_launch`、重啟後的中斷）都改成一律呼叫 `controller_cli_next_ids`，只有完全退出刻意傳 `None`（`supervisor/controller_cli.rs:234`）。所有 Domain 狀態轉換都先經過 `ensure_open`，所以不會重複開新 Run。附註：審查報告描述的那個時序（快照之後才按換帳號、接著宿主消失）沒有專屬的回歸測試；新增的 harness 測試中，`a_switch_requested_before_the_window_opened_*` 在修正前的 Supervisor 也會通過（它看得到快照裡的換帳號要求）。修正本身是無條件化，看程式碼就能確認，所以不判 FAIL。
- [PASS] R026 第 2 項（低）`begin_launch` 遇到 `end_requested_at` 已有值就直接結束 — 見 `crates/baton-domain/src/controller_cli.rs:374-379`。Domain 測試 `a_run_whose_end_was_requested_before_the_launch_intent_*` 在修正前的 Domain 會失敗（舊版會寫入 launch intent）。
- [PASS] R026 第 3 項（低）還原時中斷尚未結束的 Controller CLI — `backup.rs:437` 呼叫 `interrupt_open_controller_clis`，與其他中斷在同一個交易內。harness 測試 `after_restore_an_unfinished_controller_cli_is_not_opened` 在修正前會因為自動開窗而失敗。對應 AI-54。
- [PASS] R026 第 4 項（低）憑證有效性的規則收斂成一份 — `controller_run_active` 與 messages 的 Controller 寄件者都改用 `controller_credential_valid`，它以 `load` 載入（固定 `kind='cli'`），再交給 Domain 的 `credential_valid` 判斷。store 測試 `only_an_open_controller_cli_that_was_not_rebound_may_send_as_the_controller` 在修正前會失敗（舊版 SQL 沒有篩 `kind`、也沒看 `rebound_at`）。對應 AI-55。
- [PASS] R026 第 5 項（低）`Ending`／`SwitchWhileEnding` — 錯誤代碼仍是 `controller_cli_ending`。Domain 測試 `opening_while_the_previous_run_is_ending_*` 在修正前會失敗（舊版回 `AlreadyOpen`）。對應 AI-56。
- [PASS] R026 第 6 項（低）`ai_decisions_path` 改由 `AI_DECISIONS_RELATIVE_PATH` 組成路徑，既有路徑測試通過。
- [PASS] R027 第 1 項（中）送進模型的舊對話用語 — shim 的 `read_inbox`、`propose_decision_request` 說明，以及 API 的 `gate`、`propose_decision_request`、`reviews::requires_decision` 與 401 訊息都已改寫。搜尋 `crates/baton-mcp-shim/src`、`crates/baton-api/src` 與 `web/src`，已沒有「對話中收到」「這一輪 Controller」等字串會送進模型。只改文字、不改行為，沒有測試可接受。有對應的 AI-59（重要度中）。`api/messages.rs:98` 還有一處「那一輪」，但只是程式碼註解，不會送進模型。
- [PASS] R027 第 2 項（低）完全退出改為逐筆收集 — Worker 與 CLI 的停止以 `futures::stream::select` 合成一個串流，完成一筆就記一筆（`supervisor.rs:427-445`）；逾時的 CLI 訊息改為「Run 維持原狀」。harness 測試 `full_exit_reports_each_controller_cli_as_it_stops_even_when_another_hangs` 在修正前會失敗（舊版整批丟棄，已停止的那個會被標成 `stop_unconfirmed`）。
- [PASS] R027 第 2 項後半（NOT_FIXED）CLI 的停止沒有提前到 find 迴圈之前 — 由 AI-58 記載，理由合理：主要缺陷已修好；提前開始要重排整段停止流程，而且仍受同一個總上限約束。
- [PASS] R027 第 3 項（低）`create_file_if_absent` 寫入失敗時刪除剛建立的檔案 — 見 `create_new_with`，另有兩個單元測試：注入寫入失敗、已存在的檔案不動。對應 AI-62。
- [PASS] R027 第 4 項（低）`understanding.rs`（api 與 store）的模組文件已改寫。
- [PASS] review-notes 第 3、5 項（lifecycle 並行寫入）— 改名與讀取遇到 os error 5／32 時有上限地重試。**我自行驗證修正前會失敗**：在臨時 worktree 把 `lifecycle.rs` 換回 `cli/v1` 的版本，新測試 `writers_and_readers_survive_*` 3/3 失敗（31～42 次「存取被拒 (os error 5)」）；修正後連跑 5 次全部通過。讀者的呼叫端只有 `baton-runtime/src/lib.rs:312`，且在 `spawn_blocking` 中執行，多出的重試延遲不會卡住 async 執行緒。對應 AI-61（重要度中）。
- [PASS] review-notes 第 6 項 — 新增 `prepare_controller`，只寫 `mcp.json`，並以宿主的引數偏移做批次檔安全檢查。harness 測試 `the_run_directory_gets_the_mcp_config_but_no_worker_files` 在修正前會失敗（舊版會寫出 `prompt.md` 與 `system-prompt.md`）。另有 adapter 單元測試。對應 AI-57。
- [PASS] review-notes 第 7 項 — 搜尋整個 repo，已沒有 `ConversationNotice`、`WorkflowNote`、`SessionOwner::Conversation`、`.notices`。被刪的測試斷言只涵蓋已刪除的輸出；`gate_tests` 拿掉的重驗註記斷言，其重驗行為另有 `a_build_change_invalidates_only_the_gates_*` 等三項測試涵蓋。對應 AI-60。
- [PASS] review-notes 第 9 項 — `Store::controller_cli_accounts` 已刪除，沒有殘留的呼叫端。
- [PASS] review-notes 第 10 項 — 重啟後中斷舊 Run 時一併傳入 `next`。harness 測試 `a_switch_requested_before_a_crash_still_opens_the_new_account_when_the_host_is_gone` 在修正前會失敗（舊版中斷時不開新 Run）。對應 AI-53。
- [PASS] 沒有新缺陷、沒有弱化既有測試 — 所有狀態轉換都有終態檢查，新 Run 不會重複開；唯一索引仍然保障每個 Workspace 只有一個開著的 CLI。messages 新測試對舊輪次接受 `StaleSender` 或 `RunNotFound` 兩種結果，寬了一點，但比修正前嚴格。工作樹始終乾淨（`git status` 為 0 行）。
- [PASS] 前置條件 — 驗證期間 `cli/v1` 前進到 f662eab，新增的 4 個 commit 全部只動 `.scratch/`，不影響這次驗證。

TESTS:
- `pnpm test`：結束碼 0。Clippy、Rust、前端（30 檔、678 項）、UI E2E（4 項）全部通過，Rust 共 136 段 `test result: ok`，沒有任何 FAILED。新增的 14 項測試都在其中，結果都是 ok。
- `pnpm --filter web typecheck`：結束碼 0。
- `cargo check -p baton-desktop`：結束碼 0。
- 加做：
  - 在修正前的 `lifecycle.rs` 上跑新的 lifecycle 測試，3/3 失敗。
  - 修正後 `cargo test -p baton-store --lib lifecycle` 連跑 5 次全部通過，`cargo test -p baton-store` 127 項通過。
  - 驗證途中，臨時 worktree 共用 target 目錄，讓 cargo 沿用了修正前編出的 baton-store 執行檔，造成一次假失敗。已執行 `cargo clean -p baton-store` 重建後確認通過，臨時 worktree 也已移除。`pnpm test` 是在這之前跑的，結果不受影響。

FIXES_NEEDED: none

（不影響判定的建議：日後可以為 R026 第 1 項的時序補一個 harness 測試，重現「快照之後才按換帳號、接著宿主消失」。另外，`api/messages.rs:98` 等程式碼註解還有零星的「一輪」用語。）
exit: 0

