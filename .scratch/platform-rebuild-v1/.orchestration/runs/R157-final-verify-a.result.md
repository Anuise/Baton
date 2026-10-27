VERDICT: PASS
CHECKED:
- [PASS] R151 第 1 項【高】，verifying 後重做的 Run 不能再通過：migration 46 刪掉了 `task_verification_one_pass`（`crates/baton-store/src/lib.rs:1781-1787`）。每個 Run 最多一筆有結論的紀錄，這點仍由 `task_verification_one_conclusion` 保證（`lib.rs:1009`）。store 測試會在同一個 Task 寫入第二筆 passed，舊索引下這一步一定失敗；另外還有 harness 的 `worktrees` 測試覆蓋「整合失敗 → 重做 → 再通過」整條路徑。
- [PASS] R151 第 2 項【高】，監護日誌指向已結束的恢復事件：Store 在交易內查 `event_id` 是否已用過，用過就改用 `attempt_id` 開新事件（`crates/baton-store/src/recovery.rs:126-131`、`crates/baton-domain/src/recovery.rs:370`）。store 有 2 個測試、domain 有 1 個。測試同時確認舊事件仍是 `succeeded`，而且再匯入同一份日誌回傳空結果，不會失敗。修正前一定撞唯一約束。
- [PASS] R151 第 3 項【高】，重現案例的信任邊界：新增 `check_case_boundary`（`crates/baton-domain/src/exploration.rs:668-738`）。Command 必須和 Profile 中某個操作完全相同；帶秘密的操作也一樣；HTTP 與 Goto 只能連到 Profile 出現過的 scheme＋host＋port；網址含 `@` 或 `{` 的一律拒絕。測試涵蓋 powershell、改過參數的 node、同主機但不同操作還帶秘密、`@evil`、`8080.evil` 等繞法，修正前都會被接受。我另外確認 `Act` 的參數同樣受 `secret_keys` 檢查。
- [PASS] 第 4 項，取消意圖優先：新增 `finish_unless_cancelling`，套用在 LaunchFailed、程序已死、Worker 失敗、QuotaLimited 四條結束路徑，結束方式和 `ConfirmCancelled` 一致（Cancelled、Task 回 ready、不計預算），QuotaLimited 在取消中也不重新排隊。有 3 個 domain 測試。
- [PASS] 第 5、6 項，交接：`begin_handoff_stop` 與 `transfer_handoff` 遇到取消就放棄交接，轉移路徑在同一個交易中完成取消；建立交接和進入 stopping 時，名額計算改為扣掉即將結束的舊 Run（`handoffs.rs:118`、`:280-286`）。共 4 個 store 測試。
- [PASS] 第 7 項：`reset_at` 只記錄已觀測來源的值（`handoff.rs:195-200`）。測試證明推估值在 70%／80% 時仍維持遲滯。
- [PASS] 第 8 項：`controller_action` 的 `same_operation` 加了 `origin` 比對，`unused_answer` 共用同一個 closure，所以 Worker、Controller 兩個方向都修到了（`decision.rs:1426-1435`）。
- [PASS] 第 9、10 項：Controller 取消預算已用盡的 Task 前，要先送 `NextStepAfterBudget` 裁定（`crates/baton-api/src/controller.rs:397-403`，有 harness 測試）。預算用盡的請求只有在它就是目前用盡那一筆時才不因契約改版而過期；我確認 store 傳進去的是 `loaded.budgets`，不是空陣列。
- [PASS] 第 11～13 項：重驗不會沿用 reproduce／verify_fix 用途的驗證要求；已結束的 Task 不再觸發依據被推翻；審查 Gate 會比對 candidate 與目前的 build。我追到實際取得 build 的地方，是 `git rev-parse HEAD` 回傳的完整 SHA（`crates/baton-runtime/src/verification_driver.rs:124`），所以填 SHA 或 7 位以上縮寫的審查能正常開啟 Gate，填分支名或 `HEAD` 的不行。
- [PASS] 第 14 項：`subject_has_fix` 用目錄加整合時間判斷受驗目錄是否含修復（`crates/baton-store/src/exploration.rs:282-334`）。store 測試涵蓋整合前後、舊 worktree 與新 worktree 共 8 種組合。
- [PASS] 第 15、18 項與 review-notes #5：還原時在同一個交易內補上 `ended_cleanly_at`；manifest 與實際快照兩邊都檢查 schema 版本；`end_unfinished_runs` 也呼叫了 `ensure_compatible`；拒絕 `..`、絕對路徑與格式錯誤的雜湊。
- [PASS] 第 16 項：`read()` 改用 `unchecked_transaction` 包住整個查詢。測試在查詢中途從另一條連線插入資料，修正前兩次 COUNT 會不同。
- [PASS] 第 17、19～29 項，以及 review-notes #9 第二點、#11、#12 第一點、#17、#20～#22、#25 第一點、#26：都逐一讀過 diff 與對應測試。MarkReady 收緊後，現有呼叫端只在 Task 是 pending 時呼叫：Controller 端在 `controller.rs:360` 先檢查 pending，Web 的按鈕也只在 pending 顯示，所以不受影響。
- [PASS] NOT_FIXED 的理由：AI-463、466、467、471、472 都有條目，標示「最終 review 修正」與「審閱：未審」，理由合理。#15、#21 第二點、#25 第二點的程式在 guardian／runtime，R155 的範圍確實涵蓋。
- [PASS] 沒有刪除或弱化既有測試：在 diff 中找不到被刪除的測試函式或 assert。唯一改了預期的是 `failures_follow_the_failure_route_until_the_budget_is_used_up`，改動方向正是 review-notes #20 要的行為，而且斷言變多。`verification_handoff.rs` 只補了 revision 設定，斷言沒動；`verification_attempt` 測試只修正註解，內容沒變。
- [PASS] `ai-decisions.md` 只有新增（231 行新增、0 行刪除），新增 AI-449～AI-473，既有條目沒有被改。
- [PASS] 前置條件：驗證期間 `rebuild/v1` 從 880c580 前進到 f667321，但新增的 commit 只動 `.scratch` 底下的 run 紀錄檔，不影響這次驗證。工作樹全程乾淨。

TESTS:
- `pnpm test`：結束碼 0。Rust 全部通過；前端 30 個測試檔、636 個測試全部通過。
- `pnpm --filter web typecheck`：結束碼 0。
- `cargo check -p baton-desktop`：結束碼 0。有 1 個既有警告：`crates/baton-api/src/shared_settings.rs:20` 的 `StoreError` 沒有用到。這是 review-notes #9 第一點，屬於 R155 範圍，不是這個分支造成的。

FIXES_NEEDED: none

整合時要注意的事（不影響判定）：
- `rebuild/final-fix-c` 也用了 AI-449～AI-457，與本分支的 AI-449～AI-473 編號重疊，合併時必須重新編號，並同步更新程式註解中引用的編號。
- Web 的確認授權呼叫還沒帶上 `expected_revision: summary.changes?.base_revision`（AI-471）。在補上之前，第 28 項的保護實際上不會生效。
- 第 3 項的新限制還沒寫進 mcp-shim 的工具說明，目前 Controller 只能從錯誤訊息得知。
