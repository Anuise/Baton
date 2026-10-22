VERDICT: PASS
CRITERIA:
- [PASS] 一般使用者帳號可完成安裝與啟動。單使用者安裝的設定在 `src-tauri/tauri.conf.json:32`（`"installMode": "currentUser"`）。我以非系統管理員身分執行（`IsInRole(Administrator)` 回傳 False），`pnpm build --bundle` 產出 `target/release/bundle/nsis/Baton_0.1.0_x64-setup.exe`。接著執行 `node scripts/smoke-installer.mjs`，輸出包含「✔ 一般使用者安裝完成…（baton-desktop.exe、baton-runtime.exe、baton-guardian.exe、uninstall.exe）」與「✔ 已安裝的 Runtime 已啟動（未帶憑證回應 403；pid 37728）」，最後印出「冒煙測試通過」，結束碼 0。
- [PASS] 更新期間監護與登入啟動不會拉起 Runtime。
  - 情境測試 `crates/baton-harness/tests/installer_and_updates.rs:273` `consenting_to_an_update_suppresses_the_guardian_and_login_start_until_the_app_is_reopened` 斷言以下各點：
    - 同意更新後，生命週期檔同時有 `maintenance.kind=update` 與 `stop`。
    - 停止流程讓 2 個 Run 都變成 `interrupted/stopped_by_full_exit`，接著建立 `pre_update` 備份。
    - 監護回傳 `Finished(Suppressed(Maintenance("update")))`，`guardian_starts` 為空。
    - 重新登入後，登入啟動走的是真正的 `launcher::launch_at_login`（harness `lib.rs:499`），結果為 `LoginLaunch::Suppressed(Maintenance("update"))`，且 `spawn_count` 沒有增加。
    - 換上新版本後手動開啟，維護抑制解除；之後依 DR-01 續跑，只有開啟自動續跑的 Workspace 會派出新的 Run。
  - 同檔 `:362`（備份失敗時不套用更新，且維持抑制）與 `:490`（由 App 執行的安裝程式不重做維護）也通過。
  - 抑制的判定在 `crates/baton-domain/src/guardian.rs:159-164`：只要有維護就優先回報維護抑制。
- [PASS] 解除安裝後不留下登入啟動機碼，資料目錄保留。
  - Hook 在 `src-tauri/installer-hooks.nsh` 的 `NSIS_HOOK_PREUNINSTALL`，執行 `baton-runtime.exe maintenance prepare-uninstall`；實作在 `crates/baton-runtime/src/installer.rs:76-133`。
  - 情境測試 `:399` 與 `:455` 斷言：登記已移除、控制資料庫還在、Workspace 資料還在。
  - 實機冒煙測試的輸出為「✔ 解除安裝後 Runtime 已結束」「✔ 解除安裝後 HKCU Run 值 Baton 已移除」「✔ 資料目錄保留（…\control.sqlite）」。保留下來的生命週期檔含 `"maintenance":{"kind":"uninstall"}`，證明是我們的 hook 實際執行過，而不只是 Tauri 自己刪除了 Run 值。
  - 測試結束後我再檢查一次本機：HKCU Run 的 `Baton` 值、`HKCU\...\Uninstall\Baton`、`HKCU\Software\baton`、`baton*` 程序、`%TEMP%\baton-smoke-*`、開始功能表捷徑、`%LOCALAPPDATA%\Baton` 全部不存在，沒有殘留。
- [PASS] 以主要 seam 寫的情境測試涵蓋本票行為並通過，AI 決策也已追加。
  - `installer_and_updates.rs` 共 9 個情境測試，涵蓋：啟動時與每 24 小時檢查只通知、未設定更新來源、簽章錯誤或版本過時時不停止任何工作、更新維護、備份失敗、解除安裝（Runtime 執行中與已不在兩種情況）、更新與解除安裝互不干擾、維護與完全退出互斥。結果：`test result: ok. 9 passed; 0 failed`。
  - `ai-decisions.md` 新增 AI-362～AI-370，每條都標了重要度。

TESTS:
- `pnpm build --bundle`：成功，NSIS 安裝程式 8.00 MiB。有一個 `unused import StoreError` 警告，位置在 `crates/baton-api/src/shared_settings.rs:20`，不在本票的 diff 裡。
- `pnpm test`：結束碼 0。Rust 共 95 個 test binary 全部 `ok`，0 failed，其中 baton-domain 單元測試 612 個通過。前端 `Test Files 25 passed (25)`、`Tests 539 passed (539)`，最後印出「Rust：通過／前端：通過」。
- `node scripts/smoke-installer.mjs`：「冒煙測試通過」，結束碼 0，本機無殘留。
- `git status --short`：空的，工作區沒有被修改。

FIXES_NEEDED: none

前置條件與整合注意事項（依指示只標註，不影響判定）：
1. `git merge-base --is-ancestor rebuild/v1 HEAD` 的結束碼是 1，所以前置條件不成立。HEAD 合併到的是 `e3ae2cb`，`rebuild/v1` 目前在 `724a772`，中間還有票 42 合併以後的提交。
2. 主控合併時會遇到兩處編號衝突：
   - **Migration**：`rebuild/v1` 的票 42 已經把 `project_understanding` 定為第 37 個 migration，本票的 `maintenance`（`crates/baton-store/src/lib.rs:1349`）也排在第 37 個，需要順延成第 38 個。
   - **AI 決策編號**：`rebuild/v1` 的 AI-362～AI-370 是票 42 的條目，本票也用 AI-362～AI-370，需要改號為 AI-371 起。文件與註解中提到的這些編號要一起更新。
