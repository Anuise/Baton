驗證完成，第 31 張票判定通過。四條功能驗收條件都有對應的 Runtime harness 情境測試，全部測試與建置都通過。

**前置條件：** 開始驗證時，`git merge-base --is-ancestor rebuild/v1 HEAD` 成立（`ANCESTOR_OK`，當時 `rebuild/v1` 是 `fbc7340`）。驗證途中整合分支前進到 `6ba16c1`，多了 `b0d46a1` 與 `6ba16c1` 兩個 commit，所以目前 `rebuild/v1` 已不是 HEAD 的祖先。這兩個 commit 只改了 `.scratch/.../.orchestration/` 的 run 紀錄，與功能無關。依指示只標註，不判 FAIL。

```
VERDICT: PASS
CRITERIA:
- [PASS] 在嘗試保存、舊程序停止、新程序啟動等邊界中斷，不突破三次上限、不重複建立 writer — `crates/baton-domain/src/recovery.rs`：`begin_attempt` 先把嘗試序號 +1 並持久化；`attempts_used >= MAX_ATTEMPTS` 時拒絕開始。`on_start` 遇到停在 Attempting 的事件，會把那次嘗試記為失敗。資料庫另有保護：`recovery_attempt` 的 `CHECK (seq BETWEEN 1 AND 3)` 與 `UNIQUE (event_id, round, seq)`（`crates/baton-store/src/lib.rs` 的第 28 個 migration）。`supervisor.rs` 的 `attempt_recovery` 依序執行「Begin → 核對 → stop_old_writers → Complete/Fail」，只要有程序無法確認停止，那次嘗試就失敗，也不建立新 Run。`tick` 在恢復未完成時略過 launch/schedule，`halt_for`／`launch_halted_in` 也擋住派工（`crates/baton-store/src/lifecycle.rs`）。情境測試：`crash_at_the_attempt_saved_boundary_counts_the_attempt_and_waits_thirty_seconds`、`crash_at_the_old_worker_stopped_boundary_does_not_create_a_second_writer`、`crash_at_the_new_worker_started_boundary_reuses_the_event_and_never_runs_two_writers`。測試都斷言 `alive().len()==1` 與 spawn_count，全部 ok。
- [PASS] 退避期間重建 Runtime 不跳過期限 — 退避期限持久化在 `next_attempt_at`；`on_start` 在 Waiting 時不做任何變動；`begin_attempt` 在 `now < until` 時回 `BackoffActive`。測試 `rebuilding_during_backoff_does_not_skip_the_deadline`：先 crash_restart，再正常 restart，時鐘推進到期限前 1 ms，stop_calls 仍是 1；推進最後 1 ms 後才做第 2 次嘗試。ok。
- [PASS] 觀察期內再崩潰沿用原事件與計數 — `on_start` 遇到 Observing 且判定崩潰時，走 `fail_current`：沿用原事件 id 與 attempts_used，並設定 30 秒退避。測試 `crash_during_observation_keeps_the_event_and_count`（斷言 id 相同、attempts_used=1、失敗原因含「穩定觀察」）與 `crash_at_the_new_worker_started_boundary_*`。ok。
- [PASS] 三次失敗後重建或等待都不產生第四次；人工重試建立新輪次並保留歷史 — 第三次失敗後 `fail_current` 進入 Exhausted；`on_start` 與 `recover_after_crash` 都不處理 Exhausted；`retry` 只在 Exhausted 時接受，round+1、本輪次數歸零，舊嘗試保留（`crates/baton-api/src/recovery.rs` 的 POST `/api/v1/recovery/retry`，條件不符回 409）。測試 `three_failures_block_and_neither_rebuilding_nor_waiting_produces_a_fourth_attempt`：crash_restart、restart、各等 1 小時後，stop_calls 仍是 3、spawn 仍是 1，新工作的等待原因是 `crash_recovery_exhausted`。測試 `manual_retry_starts_a_new_round_and_keeps_history`：嘗試紀錄為 [(1,1,failed),(1,2,failed),(1,3,failed),(2,1,observing)]，並驗證兩種 409 錯誤碼。UI：`web/src/RecoveryPanel.tsx`、`StatusBar.tsx` 顯示事件、次數、退避期限與受影響工作，並提供「重新嘗試恢復」，有對應的元件測試。
- [PASS] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加至 ai-decisions.md — `crates/baton-harness/tests/crash_recovery.rs` 共 12 個情境測試，都經由真實 Axum API、真實 SQLite 與 fake Clock/ProcessControl 執行，全部 ok。ai-decisions.md 新增 AI-280～AI-286（第 2523–2580 行），每條都標了重要度。
TESTS:
- `pnpm test`（node scripts/test.mjs）→ EXIT=0，輸出「Rust：通過／前端：通過」。Rust 有 76 組 test result，合計 1211 passed、0 failed（含 crash_recovery 12/12、recovery_client、baton-domain／baton-store 的 recovery 單元測試）。前端 vitest 19 個檔案、426 個測試全部通過。
- `pnpm build` → EXIT=0，release 建置完成，產生 `target/release/baton-desktop.exe`。有 1 個警告：`crates/baton-api/src/shared_settings.rs:20` 的 unused import `StoreError`。這一行在 `rebuild/v1` 上就是這樣，本票的 diff 沒有動這個檔，不歸本票。
- 驗證後 `git status --short` 是空的，沒有改動任何追蹤中的檔案。
FIXES_NEEDED: none
```

**沒有判 FAIL、但值得留意的地方：**
1. **Ctrl-C 也算崩潰：** 正式執行檔 `crates/baton-runtime/src/main.rs` 在 Ctrl-C 或 `wait()` 結束時不會呼叫 `shutdown()`，所以不會記下正常結束。目前只有「完全退出」（有停止意圖）算正常停止，主控台的 Ctrl-C 與程序被終止都會在下次啟動觸發崩潰恢復。AI-280 已明確記錄這一點，標為高重要度。
2. **重開機也算崩潰：** 重開機後由登入自啟動拉起的 Runtime 也會被當成崩潰而開始恢復。AI-280 已記錄，跨登入的處理留給票 33。
