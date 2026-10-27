VERDICT: PASS

先確認了分支：`git merge-base --is-ancestor cli/v1 HEAD` 回傳成功，`cli/v1` 是 HEAD 的祖先。差異共 5 個 commit、19 個檔案。驗證過程沒有改動任何追蹤中的檔案，結束時 `git status` 是乾淨的。

CRITERIA:
- [PASS] 關閉 App 視窗（Runtime 仍在）不影響 Controller Run — `src-tauri/src/main.rs:1495-1500` 在使用者關閉主視窗時只把視窗藏起來，不結束 Runtime。只有完全退出（`app_quit`）才會停止。情境測試 `closing_the_app_window_leaves_the_controller_cli_running`（`crates/baton-harness/tests/controller_cli_exit_and_recovery.rs`）確認：時間推進 5 分鐘後，Run 仍是同一個且狀態是 running；沒有任何停止宿主的呼叫；開出的終端機只有 1 個；憑證還能用（200）。
- [PASS] 完全退出時 Controller 宿主被結束，Run 進入 ended，退出進度顯示這一項 —
  - 退出流程：`crates/baton-runtime/src/supervisor.rs:421-463` 先把每個開著的 Controller CLI 以「等待中」列入停止報告，再和 Worker 一起停止，同樣受 40 秒總上限約束。這段期間拿著 tick 鎖，所以不會同時開出新的視窗。
  - 結束宿主：`supervisor/controller_cli.rs` 的 `stop_controller_clis_for_exit` 結束宿主後，以 `end_code = full_exit` 結束 Run。無法確認已停止的，記為停止異常，Run 維持原狀。
  - 停止報告：`lifecycle.rs` 新增 `RunStopRecord.controller_cli` 欄位，用來標出這一項。
  - 前端：`web/src/ExitProgress.tsx:139` 把這一項標示為「Workspace · Controller CLI」。
  - 測試：`full_exit_ends_the_controller_host_and_lists_it_in_the_exit_progress` 驗證宿主被停止、狀態 ended/full_exit、憑證回 401、停止報告列出這一項，最後一筆時間線事件是 `controller_cli_ended`。`full_exit_records_a_controller_host_that_cannot_be_stopped_as_a_stop_anomaly` 驗證停止異常的情形。`ExitProgress.test.tsx` 也有對應測試。
- [PASS] Runtime 崩潰重啟：宿主不存活 → interrupted；存活 → 舊憑證失效、頁簽提示重新開啟 —
  - 查驗邏輯：`reconcile_controller_cli_after_restart` 對上一個 Runtime 實例開出的 Run 逐一查驗宿主：
    - 宿主還在：寫入 `rebind`。
    - 有回執、但宿主不在：寫入 `interrupt`，原因代碼 `runtime_restarted`。
    - 沒有回執、也找不到宿主：沿用票 03 的 `launch_outcome_unknown`。
  - 憑證失效：`crates/baton-store/src/conversations.rs:221-233` 在 `rebound_at` 有值時讓舊憑證失效。
  - 新欄位：migration `controller_cli_recovery` 加上 `continue_session` 與 `rebound_at` 兩欄（`baton-store/src/lib.rs`）。
  - 頁簽提示：`web/src/ControllerPage.tsx` 在 `rebound_at` 有值時以 `role="alert"` 提示使用者重新開啟。
  - 測試：`after_a_crash_a_vanished_host_marks_the_run_interrupted` 與 `after_a_crash_a_live_host_is_rebound_and_its_old_credential_revoked` 涵蓋兩種情形。後者驗證工具與宿主回報都回 401、不重複重新綁定、不自動開窗、不結束使用者的 CLI。另外 `an_orderly_restart_also_rebinds_a_live_controller_cli` 涵蓋沒有當掉的正常重啟。
- [PASS] 恢復後重新開啟同帳號時啟動規格含 `--continue` —
  - 判斷：`crates/baton-domain/src/controller_cli.rs:215-218` 找這個 Workspace 最近一次真的執行過（`running_at` 有值）的 Run；帳號相同就設定 `continue_session`。這裡依賴 `load` 以 `ORDER BY rowid` 排序，順序正確。
  - 組參數：`crates/baton-runtime/src/claude_adapter.rs:192-195` 依這個欄位加上 `--continue`；Worker 一律不加。
  - 測試：`reopening_with_the_same_account_after_recovery_continues_the_native_session` 與 `reopening_after_an_interruption_continues_only_with_the_same_account` 驗證同帳號會加、換帳號不加。Domain 與 Adapter 也有單元測試。
- [PASS] 以 Runtime harness 撰寫情境測試涵蓋上述行為並通過 — `crates/baton-harness/tests/controller_cli_exit_and_recovery.rs` 共 8 個測試，結果為 `test result: ok. 8 passed; 0 failed`。
- [PASS] AI 決策已追加至 ai-decisions.md — 追加了 AI-23～AI-26，每條都有重要度（中／中／中／低）與「審閱：未審」，沒有修改既有條目。

TESTS:
- `pnpm test`（Clippy `-D warnings`、`cargo test --workspace --exclude baton-desktop`、前端 Vitest、UI E2E）→ 結束碼 0。四項全部通過，Rust 測試沒有任何失敗；前端 Test Files 31 passed、Tests 682 passed。
- `pnpm build`（release 版 Runtime、監護程式與 Tauri 桌面 App）→ BUILD_EXIT=0，產出 `target\release\baton-desktop.exe`。

FIXES_NEEDED: none

不影響判定的觀察：如果使用者在 Runtime 當掉前已按了「結束」，而宿主在重啟前也已關閉，這個 Run 會被標為 `interrupted`，不是 `ended`。這符合 spec 的「不存活者標為 interrupted」，不需要修正。
exit: 0

