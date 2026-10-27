VERDICT: PASS

前置條件沒有成立：`rebuild/v1` 不是 HEAD 的祖先（`git merge-base --is-ancestor rebuild/v1 HEAD` 結束碼 1）。HEAD 少了整合分支上的票 35 合併（`b9d7fd8 merge: 票 35`）。依指示，這點不影響判定，但整合時一定會撞到兩個編號：
- **migration 編號**：兩邊都是第 23 個 migration。`rebuild/v1` 的第 22 版已經被票 35 用掉（commit `67cd631` 標為第 22 個），本票的 `external_operation` 也是第 22 版。
- **AI 決策編號**：`rebuild/v1` 已經用到 AI-237（票 35 佔 AI-227～AI-236），本票的 AI-227～AI-233 與它重疊。

CRITERIA:
- [PASS] 在程序啟動、回執、外部回應等邊界注入故障，重建後不重複啟動、不重送副作用。
  - **spawn 失敗先查證**：`crates/baton-runtime/src/supervisor.rs` 的 `launch_one` 在 spawn 回報失敗後，先以 `process_control.find` 用 command_id 找程序。找到就補寫回執；確定沒有才記為啟動失敗；查不到就留在 `starting`，不重派。
  - **launch intent 時重查准入**：寫入 launch intent（`BeginLaunch`）時在同一個交易重查准入（`crates/baton-domain/src/run.rs` 的 `admit_at_launch`）。不成立時 Run 直接結束，不啟動程序。
  - **Run 結束時標成結果未知**：還沒回報結果的外部操作，在 Run 結束的同一個交易轉成結果未知（`crates/baton-store/src/runs.rs` 的 `save` 呼叫 `settle_run_end`）。
  - **冪等鍵規則**：同一個鍵、內容不同時拒絕；已成功的操作回 `already_succeeded`（`crates/baton-domain/src/external_operation.rs` 的 `begin`）。
  - 對應的情境測試都通過（`crates/baton-harness/tests/outbox_and_external_ops.rs`）：
    - `a_spawn_that_starts_the_process_but_loses_the_receipt_is_found_by_command_id_and_never_started_twice`
    - `a_launch_that_cannot_be_proven_stays_in_reconciliation_and_is_never_redispatched`
    - `a_run_whose_authorization_is_revoked_before_launch_ends_without_starting_a_process`
    - `a_crash_after_an_external_operation_was_sent_never_resends_it_and_holds_only_the_related_task`
    - `a_lost_response_to_a_reported_result_is_safe_to_retry_and_a_succeeded_operation_is_never_sent_again`
    - `reusing_an_idempotency_key_for_a_different_request_is_refused`
    - `a_disk_that_fills_up_right_after_spawn_loses_no_process_the_receipt_is_written_once_space_returns`
  - 這些測試斷言 `spawn_count() == 1`，也斷言新 Run 登記同一個鍵時拿到 `already_succeeded`。
- [PASS] 外部操作結果未知時相關 Task 阻擋，獨立 Task 繼續。
  - `mark_unknown` 與 `hold_task`（`crates/baton-domain/src/external_operation.rs`）會建立緊急程度 high 的裁定請求，選項是 `verified_done`／`verified_not_done`，並讓相關 Task 進入 `waiting_decision`。
  - 別的 Task 用同一個鍵時，會加入同一個請求的阻擋範圍（`begin` 處理 `Unknown` 的分支）。
  - 回答裁定時，查證結果與 Task 恢復在同一個交易提交（`crates/baton-store/src/decisions.rs` 的 `resolve_by_decision`）。
  - 情境測試 `an_unknown_result_from_the_worker_blocks_its_task_while_independent_tasks_are_still_dispatched` 與 `a_crash_after_...holds_only_the_related_task` 斷言：相關 Task 是 `waiting_decision`，獨立 Task 照常 running 並 succeeded。store 測試 `another_task_hitting_an_unknown_operation_joins_the_same_decision` 也通過。
- [PASS] 磁碟滿時新派工停止，UI 顯示可處理原因。
  - **故障歸類**：`crates/baton-store/src/health.rs` 依 SQLite 錯誤代碼判斷故障種類（FULL／BUSY／LOCKED／IOERR／READONLY）。只有一次真正寫入的試寫成功，才解除故障。
  - **Supervisor 停止派工**：`supervisor.rs` 在 `check_storage`、`launch_pending`、`schedule_queue` 都會檢查 `dispatch_halted`。
  - **API 回 503**：會寫入的 API 回 503，錯誤代碼是 `storage_disk_full` 等（`crates/baton-api/src/workspaces.rs`）；`/api/v1/health` 多了 `storage` 欄位。
  - **UI**：`web/src/StatusBar.tsx` 顯示「新派工已停止」、原因與處理方式。
  - 測試通過：`a_full_disk_halts_new_dispatch_and_the_ui_shows_a_reason_it_can_act_on`、`a_database_busy_fault_is_shown_with_its_own_reason`、health.rs 以 `max_page_count` 重現真實 SQLITE_FULL 的單元測試，以及 `web/src/StatusBar.test.tsx`。
- [PASS] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md。
  - `outbox_and_external_ops.rs` 共 10 個測試，全部通過（`10 passed; 0 failed`）。
  - 測試透過真實 HTTP API 操作，用 fake ProcessControl 注入故障，用 `harness.restart()` 模擬 Runtime 重建。
  - `.scratch/platform-rebuild-v1/ai-decisions.md` 的 AI-227～AI-233 都有標重要度（例：AI-227「重要度：高」）。

TESTS:
- `pnpm test`：結束碼 0。
  - Rust（`cargo test --workspace --exclude baton-desktop`）：1046 passed、0 failed。
  - 前端（vitest）：18 個檔案、356 passed。
  - 紀錄在 `target/r087-test.log`。
- `pnpm build`：結束碼 0，產出 `target/release/baton-desktop.exe`，紀錄在 `target/r087-build.log`。
- `git status`：乾淨，沒有修改任何追蹤中的檔案。

FIXES_NEEDED: none

以下三點不影響判定，交給整合與後續的票：
1. **整合票 35 時的編號與合併**：
   - migration 要再順延為第 23 版，AI 決策改為 AI-238 起，並同步更新 `docs/development.md` 裡的「migration 22」。
   - 票 35 新增了六種裁定處理方式。`resolve_by_decision` 和 `verified` 只認得 `verified_done`／`verified_not_done` 這兩個選項，用其他方式結案時，外部操作會一直停在 `unknown`。整合時要確認這種請求只能以這兩個選項作答，否則另外處理。
2. **在驗證中的 Task 查證後會重做一次**：如果 Run 已成功、Task 在等待驗證（`verifying`），卻留下沒回報的外部操作，查證之後 Task 會回到 `ready` 重新派工，等於整件工作再做一次。這屬於設計取捨，可以考慮記成 AI 決策。
3. **因其他原因在等裁定的 Task**：如果 Task 已經因為別的原因在等裁定，它不會被加入「結果未知」請求的阻擋範圍。另一個裁定回答後它就會恢復並重新派出，要等新 Run 登記同一個鍵時才會被擋下。行為仍然安全，不會重送副作用。
