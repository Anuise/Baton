VERDICT: FAIL

本票程式碼本身符合全部驗收條件，建置與全部測試也都通過。判 FAIL 只有一個原因：我驗證到一半時 `rebuild/v1` 合入了票 13，現在 HEAD 已經不能乾淨合併回整合分支。

```
VERDICT: FAIL
CRITERIA:
- [FAIL] 前置：rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 回傳 exit=1。開始驗證時只差 86c5543（只改 execution.md 的文件）。驗證期間 rebuild/v1 於 17:34:20 合入票 13（1d706e7），後面又多了 a373f37、fb7e4ca、e2c3dd1、ab47b02。`git merge-tree --write-tree HEAD rebuild/v1` 回報 11 個檔案衝突：crates/baton-api/src/lib.rs、crates/baton-client/src/lib.rs、crates/baton-runtime/src/lib.rs、crates/baton-runtime/src/supervisor.rs、crates/baton-store/src/lib.rs、docs/development.md、src-tauri/src/main.rs、web/src/App.test.tsx、web/src/fixtures.ts、web/src/runtimeClient.ts、web/src/runtimeClient.test.ts 以外的其餘 web 衝突見上（共 11 個，含 App.test.tsx、fixtures.ts、runtimeClient.ts）。另外兩個編號也撞了：rebuild/v1 的第 16 個 migration 已是 `task_verification`，本票的 `lifecycle` 也是第 16 個；rebuild/v1 的 ai-decisions.md 已用到 AI-158，本票的 AI-148～AI-158 全部重號。
- [PASS] 關窗後 Run 持續；重開視窗連回同一 Runtime，不重複啟動 Worker — src-tauri/src/main.rs 的 `on_window_event` 收到 CloseRequested 時呼叫 `prevent_close` 再 `hide`；系統匣有狀態、「開啟 Baton」、「完全退出」三個項目（`setup_tray`）。crates/baton-client/tests/lifecycle_client.rs 的 `closing_the_window_keeps_runs_going_and_reopening_connects_to_the_same_runtime_without_another_worker` 驗證：Connection::AlreadyRunning、boot_count 不變、spawn_count 維持 1、Run 仍是 running。測試通過。
- [PASS] 停止意圖生效後，佇列重試、額度恢復與交接都不能啟動新 Run — crates/baton-domain/src/scheduler.rs 的 `place()` 一開頭就檢查 `capacity.halt`，Normal 與 Handoff 名額都擋；crates/baton-store/src/scheduling.rs 的派工、佇列重試、總覽三處都帶入 `halt_for`；crates/baton-store/src/runs.rs 在 BeginLaunch 前以 `launch_halted_in` 做最後一道檢查。harness 測試 `after_the_stop_intent_queue_retries_and_new_dispatch_requests_start_no_runs` 與 `after_the_stop_intent_quota_recovery_starts_no_runs` 通過。交接由 Domain 單元測試 `stop_intent_halts_every_placement_including_handoff_and_quota_recovery`（Normal 與 Handoff）涵蓋並通過。交接流程本身屬票 28／30，尚未實作，以後一樣會經過 place 與 BeginLaunch 的檢查。
- [PASS] 退出流程在 40 秒內結束，無法確認停止的程序被記錄為停止異常 — crates/baton-runtime/src/supervisor.rs 的 `run_full_exit` 以 `deadline_at`（requested_at + 40 秒）用 `timeout_at` 同時限制等 tick 鎖與等停止結果的時間，逾時的 Run 在 `finish_exit` 記為 `stop_unconfirmed`，並保留寫入所有權。以下測試通過：`full_exit_ends_within_its_deadline_and_records_unconfirmed_processes_as_stop_anomalies`（still_alive、stop_unconfirmed、stop_failed、force_killed）、`full_exit_is_not_delayed_by_a_cancellation_stuck_in_stopping_its_process`、`full_exit_still_ends_within_its_deadline_when_another_supervisor_step_never_returns`。預設寬限斷言為 30 秒 + 10 秒（lifecycle.rs:221-222）。停止進度與異常顯示在 web/src/ExitProgress.tsx。
- [PASS] Workspace A 開、B 關時，重開後只有 A 的合格工作自動恢復 — `only_the_workspace_with_auto_resume_resumes_its_work_after_reopening` 通過：設定預設 false；核對前 resume 回 409 stop_intent_active；之後 A 是 running、B 是 ready 且原因為 awaiting_resume；按「繼續工作」後 B 才派出。核對流程見 `reopen_if_pending`、`reconcile_after_exit`，由 `reopening_reconciles_processes_the_exit_could_not_confirm_before_resuming` 涵蓋。
- [PASS] 完全退出不會把 Task 標為取消或完成 — `full_exit_never_cancels_or_completes_tasks` 通過：running 退回 ready，blocked、verifying、ready 維持原狀，task 表中沒有 cancelled 或 completed。Run 以 interrupted／stopped_by_full_exit 結束（AI-148）。
- [PASS] 以 Runtime harness 撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加 — crates/baton-harness/tests/lifecycle.rs 有 13 個情境，全部通過。ai-decisions.md 新增 AI-148～AI-158，都標了重要度，但編號與 rebuild/v1 衝突（見第一條）。
- [PASS] spec D-25 停止意圖雙寫 — crates/baton-api/src/lifecycle.rs 的 `request_exit` 先寫資料庫，回應前再寫生命週期檔；`write_lifecycle_file` 以 tmp + sync_all + rename 原子寫入；啟動時 `reconcile_stop_intent` 遇到不一致取保守者；檔案毀損也視為停止。`a_stop_intent_found_only_in_the_lifecycle_file_is_treated_as_stopped` 通過。
TESTS:
- `pnpm install --frozen-lockfile` → 成功
- `pnpm build` → exit 0（baton-runtime release、web tsc + vite build、baton-desktop release 都成功編譯）
- `pnpm test` → exit 0。Rust：759 passed、0 failed、3 ignored（含 lifecycle.rs 13/13、lifecycle_client.rs 2/2）；Web（vitest）：15 個測試檔、248 個測試全部通過。
- 未執行 `node scripts/smoke-desktop.mjs`：它會開啟視窗，不在 `pnpm test` 範圍內。
FIXES_NEEDED:
1. 把最新的 rebuild/v1（目前 ab47b02，含票 13 的 1d706e7）合併進本分支，解決上面 11 個檔案的衝突，保留雙方功能：票 13 的驗證／成果接受，與本票的生命週期、系統匣、完全退出。
2. 把本票的 `lifecycle` migration 移到 `task_verification` 之後，改為第 17 個；檢查有沒有測試或文件寫死 migration 數量或版本。
3. 把本票在 ai-decisions.md 的條目從 AI-148～AI-158 順延到 AI-159 之後（不要改 rebuild/v1 既有的條目），並同步更新程式碼註解、docs/development.md、票面中引用的 AI 編號，例如 supervisor 註解與 AI-157 的引用。
4. 合併後重新執行 `pnpm build` 與 `pnpm test`，確認全部通過，再用 `git merge-base --is-ancestor rebuild/v1 HEAD` 確認祖先關係。
5. 追加一條 AI 決策，記錄這次整合（migration 與 AI 編號順延）。
```

另外有兩個低風險觀察，不列入 FAIL：
- API 的 `request_exit` 與 Supervisor 的 `mirror_lifecycle_file` 都用同一個 `baton-lifecycle…tmp` 暫存檔名，兩者同時寫入時可能有一方 rename 失敗。目前只記日誌、後續還會再鏡像，影響很小。
- `run_full_exit` 裡的 `process_of`（ProcessControl.find／ensure_managed）沒有受 40 秒期限約束。如果 find 本身卡住，退出可能超過 40 秒。
