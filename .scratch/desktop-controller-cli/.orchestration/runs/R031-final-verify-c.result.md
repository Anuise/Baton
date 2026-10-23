VERDICT: PASS
CHECKED:
- [PASS] 前置條件：`cli/v1` 在 merge-base `1690229` 之後只多一個 commit `9701e13`（只改 `.scratch`，記錄 R030），不影響這次驗證；工作樹乾淨，我沒有改動任何檔案。
- [PASS] R028 #1【中】／review-notes #1、#2、#5（harness 的當掉模擬）— `fakes.rs` 在 `hold_next_removal`、`hang_next_artifact_write_at`、`CommandRunner::hold_next`、`hang_next_http` 這四個停住點加了通知。它們都在走到停住點時呼叫 `notify_one`，會保留一次通知，所以「先通知、後等待」也不會漏接。`lib.rs:457` 新增 `supervise_until`：以 `select!` 同時跑 supervise 與等待；supervise 自己跑完就 panic；20 秒是保險逾時。審查點名的 12 個檔案都改了，`handoff`、`runs`、`sessions`、`outbox` 裡的 `supervise_until_crash` 全部刪掉。用 grep 確認 harness 測試已經沒有包住 `supervise` 的毫秒級 `timeout`。斷言沒有改：原本 `assert!(timeout.is_err())` 的意思，現在由 `supervise_until` 裡「supervise 跑完就 panic」承擔。`wait_for_stop_call` 與 `wait_for_spawn_hang` 是既有方法，判斷的條件都是在停住之前就已寫入的狀態。加壓：`worktree_cleanup`、`worktrees`、`verification_handoff`、`handoff`、`runs`、`sessions` 每支 2 份同時跑，共 2 輪 24 次，全部通過。`docs/development.md` 已同步，AI-50 已記錄。
- [PASS] R028 #2【中】時間線的 Controller CLI 帳號 — `TimelinePage.tsx` 會讀 `listProfiles`。`controller` 類別的事件顯示「帳號：名稱」（對不到就顯示 id）、「Controller CLI：aggregate_id」，有結束代碼時也顯示。我確認 `timeline.rs:122` 只把 `controller_cli` aggregate 歸到這個類別，不會誤套到其他事件。`TimelinePage.test.tsx` 新增的 2 個測試都斷言「帳號：」字樣，修正前的版本沒有輸出這段字，所以一定會失敗。typecheck 也確認 App 傳入的 client 型別相容。AI-51 已記錄。
- [PASS] R028 #3【低】／review-notes #8（前端輪詢測試）— `ControllerPage` 與 `AccountsPage` 不再用 `shouldAdvanceTime`，改用完全的假計時器加 `advanceTimersByTimeAsync`。檢查點改成 `interval-1` 與 `+1`，比原本的「留一秒餘裕」更嚴。`ExplorationPage` 改成等分區出現再斷言。`test-setup.ts` 把 findBy／waitFor 的等待上限放寬到 3 秒，斷言內容沒變。AI-52 已記錄。
- [PASS] R028 #4【低】帳號清單讀取失敗的提示 — 提示從只在「未開啟」時顯示，移到開著與未開啟都會顯示的位置（`ControllerPage.tsx:230`）。新增的測試斷言「開著時」有 `alert`，在舊版本只會出現在未開啟分支，所以修正前會失敗。
- [PASS] R028 #5【低】GLOSSARY — 四個條目都更新了，內容與審查指出的實作一致：MCP shim 的 `controller-host`、`wait` 與 `BATON_MCP_SHIM`；Controller 憑證在結束或 `rebound_at` 時失效；等待原因的 `controller_account`；Account Profile 的 `config_dir_origin`。
- [PASS] R028 #6【低】`open_controller_cli` — 現在會先確認這次的 Run 已是 `running`，再以它的 `command_id` 用 `rfind` 找開窗紀錄。新增的 `should_panic` 測試在舊版本會默默拿到上一次開窗的憑證、不會 panic，所以修正前會失敗。
- [PASS] NOT_FIXED — 分給 final-c 的第 1、2、5、8 項全部處理了；第 4 項由主控記為已完成。修正 run 新發現的 `VerificationPage` 不穩定測試不在分派範圍內，已記為 review-notes #11，這樣處理合理。
- [PASS] 有沒有引入新缺陷或弱化測試 — 沒有刪掉任何測試案例。改動的只有等待方式（findBy／waitFor、settle），斷言內容都沒變。

另有兩點不影響判定的小問題：
- `verification_handoff.rs:5` 與 `verification_takeover_blocked.rs:8` 檔頭的說明還寫「以 `timeout` 丟棄那次處理」，和新寫法不一致。
- `run_process_release` 那支測試改成輪詢「快的已經是 cancelled」，如果停止被改成一個接一個執行，它要等到 20 秒的保險逾時才失敗，錯誤訊息也比原本籠統。這是修正 run 自己點出的取捨，可以接受。

TESTS:
- `pnpm test` 沒有整支一次跑，因為一次跑會超過前景 600 秒的上限。我照 `scripts/test.mjs` 的內容拆成幾段，全部在前景跑完：
  - `cargo clippy --workspace --all-targets -- -D warnings`：通過。
  - `cargo test --workspace --exclude baton-desktop --exclude baton-harness`：通過。
  - `cargo test -p baton-harness`：分兩批跑（lib 加前 34 個測試程式；後 34 個測試程式），合計 683 個通過、0 個失敗、6 個 ignored（UI E2E 由下面另外跑）。doc 測試也通過。
  - `pnpm --filter web test`：30 個檔、678 個測試通過。
  - `pnpm test:e2e`：4 個旅程通過。
- `pnpm --filter web typecheck`：通過。
- `cargo check -p baton-desktop`：通過。
- 加壓：6 支 harness 測試程式，每支 2 份同時跑，共 2 輪 24 次，0 失敗。

FIXES_NEEDED: none
exit: 0

