票 36 驗證通過，三條驗收條件全部 PASS。`rebuild/v1`（38b6eb9）是 HEAD 的祖先，驗證結束前再查一次仍然成立。建置和全部測試都在前景跑完，工作目錄保持乾淨，沒有修改任何追蹤中的檔案。

你特別要求確認的兩項舊行為改動，都和 spec 一致，舊測試也沒有被削弱：

- **無法收集的檢查改為 inconclusive**：符合 spec 使用者故事 70，也就是證據不足不算通過、也不算產品缺陷。舊的 Domain 測試只把預期值從 `Failed` 改成 `Inconclusive`，仍然是精確斷言（`crates/baton-domain/src/verification/tests.rs:231,244`）。真正的未通過（結束碼不符、檔案不存在、雜湊不符）仍判為 `failed`。票 13 的驗收條件不受影響。
- **程序死亡改為退避後重派**：符合 spec D-16 的指數退避，以及 D-15 的「在預算內重派」可由 Controller 自主處理。兩支舊情境測試（`acceptance.rs:367-368`、`outbox_and_external_ops.rs:300-301`）各只多加一行把時鐘推過 1 分鐘，原有斷言一條都沒動。

```
VERDICT: PASS
CRITERIA:
- [PASS] 三種計數互不影響；交接不增加任何計數 — 每種預算的計數與上限都分開處理：crates/baton-domain/src/budget.rs:55-61 定義上限 3/2/5，:167-207 的 charge 只加被計入的那一種，:211-222 的 reset 只重設用盡的那一種。各種結束的計入方式：Finish Failed 計入實作失敗；LaunchFailed 與 ReconcileLease 觀察到 Dead 計入基礎設施；QuotaLimited、Interrupt、取消、准入被拒都不計入（crates/baton-domain/src/run.rs 的 finish 呼叫，charge 參數為 None）；Supervisor 在 Adapter 回報 RateLimited{Exhausted} 之後，把以失敗結束的 Run 判為 QuotaLimited（crates/baton-runtime/src/supervisor.rs:765-781）。驗證端：未通過計入實作、不完整計入驗證、契約在派工後變更不計入（crates/baton-domain/src/verification.rs，decide 中的 charge match）。情境測試 `the_three_counters_do_not_affect_each_other` 依序得到 (0,0,1)→(1,0,1)→(1,1,1)→(1,2,1)，核准後為 (1,0,1)；`quota_handoff_recovery_and_cancellation_never_increase_any_counter` 走過突然限流換帳號、Runtime 崩潰恢復、使用者取消，計數始終不變，只有檢查未通過那次記為 (1,0,0)。Domain 單元測試 `each_kind_is_exhausted_only_after_its_own_limit`、`quota_limits_recovery_interruptions_and_cancellations_never_charge_any_budget` 也通過。
- [PASS] 預算用盡時 Task blocked 並出現裁定請求 — run.rs 的 finish 與 verification.rs 的 decide 在用盡時把 Task 轉為 Blocked，並呼叫 decision::budget_exhausted_request 建立請求（authority／next_step_after_budget、blocking=Task、選項 approve/reject；crates/baton-domain/src/decision.rs）。請求與預算列在同一個交易寫入（crates/baton-store/src/budgets.rs 的 save_budget、runs.rs save、verification.rs）。三種預算的用盡都有情境測試，並共用 `assert_blocked_by_budget`，它經 HTTP 斷言 Task 為 blocked、budget.exhausted 正確、只有一筆待處理請求、matter 正確、blocking 指向該 Task：實作見 `implementation_failures_are_fixed_three_times_then_the_task_is_blocked_with_a_decision_request`，另斷言阻擋的 Task 無法派工；基礎設施見 `infrastructure_retries_back_off_exponentially_up_to_ten_minutes_then_block`，退避依序 1/2/4/8/10 分鐘，用盡後推進 60 分鐘仍沒有第 7 個 Run；驗證見 `the_three_counters_do_not_affect_each_other`。核准與駁回分別見 `approving_the_budget_decision_resets_only_that_counter_and_lets_the_task_continue`、`rejecting_the_budget_decision_keeps_the_task_blocked`。Task 詳情的 API 有 budget 欄位（crates/baton-api/src/tasks.rs:128），Web 卡片顯示三種計數（web/src/TaskBoard.tsx 的 BudgetStatus）。
- [PASS] 以主要 seam（Runtime harness，D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加至 ai-decisions.md — crates/baton-harness/tests/retry_budgets.rs 有 6 個測試，全部經真實 HTTP API、fake Clock、fake Adapter／ProcessControl／CommandRunner 執行，結果「6 passed; 0 failed」。.scratch/platform-rebuild-v1/ai-decisions.md 新增 AI-273～AI-279，每條都標了重要度（高 1、中 4、低 2），內容涵蓋計入分類、inconclusive 的定義、計數語意與退避起點、用盡請求的形狀、突然限流的判定、資料形狀與整合順延。
TESTS:
- `pnpm test`（= with-msvc cargo test --workspace --exclude baton-desktop + pnpm --filter web test）→ EXIT=0；Rust 合計 1181 passed、0 failed（另有 ignored 的 live 測試）；Vitest「Test Files 18 passed (18)、Tests 409 passed (409)」。
- `pnpm build` → EXIT=0，建出 target/release/baton-desktop.exe。
- 唯一的編譯警告是 crates/baton-api/src/shared_settings.rs:20 的 unused import `StoreError`，該檔不在本票 diff 內，是既有問題。
- 前置條件：`git merge-base --is-ancestor rebuild/v1 HEAD` → 成立（rebuild/v1 = 38b6eb9）；驗證結束前再查一次仍然成立。
- 舊行為改動（使用者交代要確認的兩點）：見上方說明，與 spec 一致，斷言未弱化。
FIXES_NEEDED: none
```

另外有三點不影響判定，可以之後再看：

1. **註解錯位**：`crates/baton-store/src/scheduling.rs:286-289` 新增的 `backoff_budget` 插在 `failed_check` 的 doc 註解和函式本體中間。結果 `failed_check` 的說明變成掛在 `backoff_budget` 上，`failed_check`（:293）反而沒有說明。
2. **格式小問題**：`ai-decisions.md:2513` 的 `### AI-279` 前面少一行空行。
3. **重驗沒有延遲**：驗證不完整時，Supervisor 下一次處理就馬上重驗，所以實際執行時兩次重驗可能在幾秒內用完、立刻送出裁定。這是 AI-274 刻意的決定，spec 只對基礎設施重試要求退避。屬於日後可以調整的設定，不是缺陷。
