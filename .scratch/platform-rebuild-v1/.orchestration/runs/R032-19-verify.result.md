VERDICT: PASS

本票的五條驗收條件都通過。`pnpm test` 與 `pnpm build` 都實際跑過、全部成功。`rebuild/v1` 確認是 HEAD 的祖先。驗證全程唯讀，結束時 `git status --porcelain` 是空的。

CRITERIA:
- [PASS] 兩個帳號同時執行不同 Task，Run 與帳號關聯正確 — 派工時 Scheduler 選定帳號，同一個交易裡把 `profile_id` 寫進 Run 和 `run_created` 事件（`crates/baton-domain/src/run.rs` 的 `dispatch`；`crates/baton-store/src/runs.rs` 的 migration 10 新增 `run.profile_id`）。選帳號規則是：從可用帳號中挑進行中 Run 最少的，同數取較早建立的（`crates/baton-domain/src/scheduler.rs` 的 `place`）。情境測試 `crates/baton-harness/tests/scheduling.rs:285` 確認：兩個 Task 都回 201，分別給帳號 1、2；`GET runs/{id}` 與 `GET /api/v1/scheduling` 的 `accounts[].runs` 都指向正確的 Task；Supervisor 啟動 2 個程序；結束後只有該帳號的計數歸零。`:341` 確認最少者優先。帳號頁顯示每個帳號的進行中 Run（`web/src/AccountsPage.tsx` 的 `AccountLoad`）。
- [PASS] 達到每帳號或全域上限時新 Task 排隊並顯示原因 — 派工端點沒有名額時回 202 `{queued:true, reasons}`，Task 維持 ready 並寫入 `dispatch_request` 佇列（`crates/baton-api/src/runs.rs` 的 `dispatch`；`crates/baton-store/src/scheduling.rs` 的 `schedule`）。每帳號上限見 `scheduling.rs:356`：原因是 `capacity/account_capacity_full` 並帶 `profile_id`，名額空出後同一次 tick 自動派出並啟動。全域上限見 `:405`：`global_capacity_full`。總覽 `queue[]` 帶順位與原因；工作面板顯示「排隊中（第 N 位）」與分組的原因（`web/src/TaskBoard.tsx`）。
- [PASS] 交接保留槽不被一般派工佔用 — 可用帳號數 ≥3 時，`normal_limit = global - 1`，一般派工（`SlotKind::Normal`）只能用 5 個名額（`scheduler.rs` 的 `CapacityLimits::normal_limit` 與 `place`）。`scheduling.rs:443` 確認：3 個帳號跑 5 個 Run 時，第 6 個 Task 回 `handoff_slot_reserved`，running 維持 5，Supervisor 也不會派出。`:475`、`:491`、`:511` 覆蓋邊界：只剩保留槽、少於 3 個帳號不保留、未核對的帳號不計入。Domain 測試 `scheduler/tests.rs:403,417` 確認 `SlotKind::Handoff` 可以用到第 6 個名額，`:792` 起是性質測試。設定頁 `web/src/SchedulingSettings.tsx` 可在兩組預設間切換：切換後立刻重新排程（`:527`），已在執行的 Run 不受影響（`:551`），設定重啟後保留（`:927`）。
- [PASS] 阻擋原因區分額度、授權、相依與狀態不明 — `WaitCategory` 分成 capacity、quota、authorization、dependency、state_unknown、account 六類（`scheduler.rs`）。各類的情境測試：
  - 額度 ≥75% → `quota/quota_near_limit`（`:627`、`:655`），未觀測不阻擋（`:674`）
  - 撤銷能力 → `authorization/capability_not_granted`（`:599`）
  - 相依未完成 → `dependency/dependencies_unfinished`（`:570`）
  - 帳號未核對 → `state_unknown/profile_unverified`（`:700`）
  - 帳號未登入 → `account/profile_not_logged_in`（`:726`）
  - 多重原因各自歸類、各自指向正確帳號（`:786`）
- [PASS] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加至 ai-decisions.md — `crates/baton-harness/tests/scheduling.rs` 透過 Runtime API 搭配 fake Port（Clock、QuotaObserver、ProcessControl、ClaudeEnvironment），共 32 個測試全數通過。`.scratch/platform-rebuild-v1/ai-decisions.md` 新增 AI-95 到 AI-102，每條都標了重要度（高 2、中 2、低 4）。

TESTS:
- `pnpm test`（`cargo test --workspace --exclude baton-desktop` 加上 `pnpm --filter web test`）→ 結束碼 0，Rust 與前端都通過。
  - Rust 全部 test result 都是 ok、0 failed，有 3 個 ignored（`live_` 開頭的手動測試）。
  - 本票的情境測試 `tests\scheduling.rs` 32 個通過，`baton-client` 的 `tests\scheduling_client.rs` 3 個通過。
  - 前端 vitest：11 個檔案、151 個測試通過。
- `pnpm build` → 結束碼 0。`baton-runtime` 的 release 建置成功；`tsc --noEmit && vite build` 成功；產出 `target\release\baton-desktop.exe`。

FIXES_NEEDED: none

以下是非阻擋的觀察，供日後參考，不需要在本票修正：
- 排隊期間如果出現不能靠等待解決的檢查失敗（例如引用失效 `reference_invalid`、`execution_conflict`），`failed_check` 會把它歸為 `state_unknown`（`crates/baton-store/src/scheduling.rs` 的 `failed_check`）。歸類略嫌寬，但不影響驗收條件。
- spec D-14 寫的是「帳號數 ≥3」，實作是以「可派工的帳號數」來判斷。這已記錄在 AI-97，也有測試 `:511` 覆蓋。
