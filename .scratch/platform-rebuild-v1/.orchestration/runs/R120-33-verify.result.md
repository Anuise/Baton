```
VERDICT: PASS
CRITERIA:
- [PASS] 兩項新設定預設關閉，與 DR-01 設定分開保存與顯示 — 「跨登入自動續跑」與「完全退出後自動續跑」存在兩個不同欄位（migration 35 `cross_login_startup`：crates/baton-store/src/lib.rs:1312-1328，新增 `workspace_lifecycle.auto_resume_across_login`，預設 0；「登入時自動啟動」另存一列 `login_startup`）；Domain 預設值在 crates/baton-domain/src/login.rs（`LoginStartup::default`，enabled=false）與 lifecycle.rs 的 `WorkspaceLifecycle::new`；兩者有各自的 API 端點 `PUT /api/v1/lifecycle/login-startup` 與 `PUT /api/v1/workspaces/{id}/login-resume`（crates/baton-api/src/lifecycle.rs）；UI 方面，登入時自動啟動在獨立的 LoginStartupSettings.tsx 區塊，兩個自動續跑在 WorkspaceSidebar.tsx 各有自己的勾選框。測試 `both_new_settings_default_to_off_and_are_kept_and_shown_separately_from_the_exit_setting`、`login_startup_follows_the_preference_in_windows_…` 與 LoginStartupSettings.test.tsx 皆通過。
- [PASS] 完全退出後重新登入不啟動 Runtime、監護或工作 — launcher.rs 的 `launch_at_login` 先讀生命週期檔，有停止、維護抑制或檔案毀損就直接回傳 `Suppressed`，main.rs 隨即結束。資料庫有停止意圖但檔案遺失時，`Runtime::start_with` 的 `login_start && reopen_from.is_some()` 會回傳 `SuppressedByStopIntent`，並把停止意圖補回生命週期檔（crates/baton-runtime/src/lib.rs）。`cross_login` 在停止意圖生效時不做任何處置，之後手動開 App 依 DR-01 處理（reconcile 不會在啟動時清除停止意圖，所以不會改用跨登入設定）。用真實執行檔驗證的測試 `suppressed_login_launch_exits_without_starting_runtime_or_guarding`（guardian）與 `login_start_refuses_to_serve_when_only_the_database_remembers_the_full_exit`（runtime），以及 harness 的 `after_full_exit_signing_in_again_…`、`a_stop_intent_only_in_the_database_…`、`maintenance_suppresses_the_login_start` 皆通過。
- [PASS] Workspace 跨登入設定相反的情境測試不會互相誤用 — `cross_login` 只看 `auto_resume_across_login`；`reopen` 只解除 `paused_by == FullExit` 的暫停（crates/baton-domain/src/lifecycle.rs）。harness 測試 `signing_in_again_resumes_only_workspaces_with_cross_login_resume` 設定 A（跨登入開、退出後關）與 B（相反）：只有 B 以 `new_login` 暫停、等「繼續工作」，A 以新 Run 續跑。另外 `after_full_exit_…` 確認完全退出後跨登入設定無法繞過 DR-01，`an_unverified_login_resumes_no_workspace_regardless_of_settings` 確認登入識別無法核對時一律暫停。皆通過。
- [PASS] 登入自啟動與手動開 App 同時發生時只有一個控制平面 — launcher 先檢查是否已有 Runtime（`AlreadyRunning`），多出來的 Runtime 會因單實例鎖自行結束。用真實 baton-runtime.exe 互相競爭的測試 `login_start_racing_the_app_leaves_exactly_one_runtime_serving` 通過，boot_count=1，多出來的程序以成功碼結束。harness 的 `login_startup_and_a_manual_app_open_at_the_same_time_leave_one_control_plane` 通過：只開機一次，跨登入處置只做一次，再次執行 launcher 時不會啟動第二個 Runtime 或第二個監護。
- [PASS] 以主要 seam 撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加 — crates/baton-harness/tests/cross_login.rs 共 10 個情境測試，經由 Runtime harness 與 fake Port 測試（spec D-04），全部通過，另含「重新登入不重設恢復額度與阻擋」的 `signing_in_again_does_not_reset_recovery_attempts_or_blocks`。ai-decisions.md 已追加 AI-350（高）、AI-351～AI-353（中）、AI-354～AI-355（低），每條都標了重要度。
TESTS:
- 前置條件：`git merge-base --is-ancestor rebuild/v1 HEAD` 確認 rebuild/v1（172280b）是 HEAD 的祖先。
- `pnpm test`：結束碼 0。Rust 全部通過，其中與本票相關的有 cross_login 10/10、login_start_process 2/2、login_launcher_process 1/1、domain login::tests、launcher 單元測試，以及 windows_login 的真實 HKCU 測試。前端 vitest 22 個檔、494 個測試全部通過。
- `pnpm build`：結束碼 0，產生 release 版的 baton-runtime、baton-guardian、baton-desktop。唯一的警告是 baton-api/src/shared_settings.rs:20 的 `StoreError` 未使用，這個檔案不在本票 diff 內，是既有問題。
- 登錄檢查：測試前後各匯出一次 HKCU\…\Run 的所有值，內容完全相同（SAME=True），沒有殘留任何 `Baton*` 或 `BatonTest-*` 值，使用者原有的 15 個值都沒有被動到。測試結束後沒有殘留的 baton-* 程序；`git status` 乾淨。
FIXES_NEEDED: none
```

非阻擋觀察（不影響判定）：
- 如果登錄中已有名為 `Baton` 的值，但命令列和本程式組出的不同（例如舊的安裝位置），而偏好又是關閉，`sync_login_startup` 會把它視為「未登記」，不會移除這個舊值。
- 以系統管理員權限（UAC 提升）啟動時，登入識別可能和一般權限下取得的不同，實作者已在 windows_login.rs 標為 Validation Required。
