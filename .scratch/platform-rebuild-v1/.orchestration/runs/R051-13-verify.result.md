驗證完成。五條驗收條件都成立，Rust 與前端測試也全部通過。但我找到一個本票新增的缺陷：某個 Task 正在驗證時，若使用者封存它所屬的 Workspace，Runtime 每秒會重跑一次驗證命令，不會停。因此判定不通過，只需要一個小修正。

```
VERDICT: FAIL
CRITERIA:
- [PASS] 只有文字成功或程序正常退出時 Task 不會變成 completed — Run 成功結束只會把 Task 推到 verifying（crates/baton-domain/src/run.rs:592）。整個程式中只有 crates/baton-domain/src/verification.rs:587 會把 Task 設為 Completed，而且必須由 decide() 判定通過。task.rs:1273 斷言 Running 不能直接到 Completed。情境測試 acceptance.rs:144（Worker 用 report_progress 宣稱完成、程序正常退出、再跑 3 次 supervise）結果停在 verifying，也沒有任何驗證紀錄，通過。
- [PASS] 過期 Run 的成果回報不能推進 Task — 成果接受交易的三項校驗在 verification.rs:487-508，依序是 Run 與世代、契約版本、manifest，任一不符就什麼都不寫。acceptance.rs:329 的情境：世代 1 已被取代後，它登記成果得到 409 run_report_stale；它回報結束時 Task 仍是 running；以世代 1 為基礎的接受得到 409 acceptance_run_stale；改用世代 2 的基礎才 200 → completed。acceptance.rs:418 驗證契約與 manifest 過期都回 409。acceptance.rs:454 是自動檢查與手動接受同時進行的情境，結果只留下一筆紀錄。資料庫另有唯一索引：每個 Run 最多一筆紀錄、每個 Task 最多一筆通過（crates/baton-store/src/lib.rs migration task_verification）。
- [PASS] 確定性檢查失敗時 Task 回到可修正狀態並保留證據 — 未通過時 Task 回到 Ready（verification.rs:588）。證據包含每項檢查的結果、輸出結尾與 manifest，存在 task_verification 表；這張表只能追加，觸發器禁止 UPDATE 與 DELETE。acceptance.rs:218 驗證結束碼 101 加上輸出 → ready，紀錄保留結束碼與輸出，工作面板 board.verifications 也看得到；重新派出世代 2 並通過後，兩筆紀錄都還在。acceptance.rs:255 和 :276 分別驗證成果檔被竄改、命令無法啟動或檔案不存在時也回到 ready。
- [PASS] 使用者手動接受需經桌面憑證，並記錄於事件 — POST …/verification/accept 屬於管理 API，掛在 guard 底下（crates/baton-api/src/lib.rs）。接受者取自已驗證的憑證指紋（crates/baton-api/src/verification.rs:82）。acceptance.rs:371 驗證：Worker 的 Agent 憑證、另一張 Agent 憑證、沒有憑證，三種都得到 403/401，Task 不動；用桌面憑證接受 → 200 completed，並寫入 task_verification_recorded 事件，內容含 accepted_by 指紋，而且不含憑證本身；重複接受得到 409 task_not_verifying。
- [PASS] 以 Runtime harness（D-04）撰寫情境測試並通過；AI 決策已追加 — crates/baton-harness/tests/acceptance.rs 的 11 個情境測試全部通過，都透過真實的 Axum API、SQLite 與 Supervisor，只把 Port 換成 fake。ai-decisions.md 新增 AI-128～AI-135，numstat 為 +72/-0，只有追加，沒有改既有條目。
- 前置條件：`git merge-base --is-ancestor rebuild/v1 HEAD` 回傳 ANCESTOR_OK，表示整合分支已合併進來。

TESTS:
- `pnpm test` → EXIT=0，結果「Rust：通過」「前端：通過」。共 53 個 cargo 測試目標全數 ok，其中 harness acceptance 11 passed、baton-domain lib 256 passed、verification_client 1 passed；vitest 13 個測試檔、221 個測試全數通過。
- `pnpm build` → EXIT=1，原因是環境問題：寫入 target\release\deps\…\full.rmeta 時發生 os error 112（磁碟空間不足），C: 剩 0 bytes。失敗前 baton-runtime 的 release 建置已經完成，web 的 `tsc --noEmit && vite build` 也成功（built in 1.04s），卡在 tauri 編譯 windows crate。
- 補充檢查：`scripts/with-msvc.ps1 cargo check -p baton-desktop` → Finished，桌面殼新增的 task_verification 和 task_accept 指令可以編譯。
- 工作樹全程乾淨（git status 沒有輸出）。

FIXES_NEEDED:
1. 封存的 Workspace 中，驗證命令會被無限重跑。`Store::pending_verifications`（crates/baton-store/src/verification.rs:88-115）和 `verification::pending_check`（crates/baton-domain/src/verification.rs:318）都沒有排除已封存的 Workspace，而 `decide()` 會在第一步就用 WorkspaceArchived 拒絕（verification.rs:474）。封存 API（crates/baton-api/src/workspaces.rs:143）不檢查 Task 狀態，所以使用者可以封存一個有 verifying Task 的 Workspace。結果是：背景迴圈每秒（BACKGROUND_INTERVAL，supervisor.rs:49）重新執行契約宣告的命令（每次最長 10 分鐘，可能有副作用），提交時被拒絕、寫一行日誌，然後不斷重複。
   修正方式：在 `pending_check`（或 `pending_verifications` 載入 Workspace 之後）跳過 status 為 Archived 的 Workspace，讓它不再成為待驗證項目；`VerificationView.automatic` 也要相應變成 false。
2. 在 crates/baton-harness/tests/acceptance.rs 新增一個情境測試：Task 宣告命令檢查，Run 成功後 Task 進入 verifying，在 supervise 之前就封存 Workspace，接著跑 supervise 多次，斷言 `command_runner.calls()` 是空的，而且沒有任何驗證紀錄。先確認這個測試在修正前會失敗，修正後通過。
3. 在 ai-decisions.md 追加一條決策：封存的 Workspace 中，verifying 的 Task 不再自動驗證（重要度：中）。
4. 修正後重跑 `pnpm test`；磁碟空間足夠時再跑 `pnpm build`。這次 build 失敗是因為磁碟已滿，不是程式碼問題。
```
