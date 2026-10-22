VERDICT: PASS
CRITERIA:
- [PASS] 突然限流後由另一帳號從檢查點接手。證據如下：
  - 情境測試 `crates/baton-harness/tests/sudden_limit.rs:223-271`：帳號 A 回報 `Exhausted`，CLI 以失敗結束，之後：
    - 門檻變成 `draining/limit_reached`。
    - `quota_observation` 有保存原始事件（L238-245）。
    - 舊 Run 以 `interrupted/handed_off` 結束。
    - 交接 `phase=transferred`，`checkpoint.summary` 是最近的檢查點（L253）。
    - successor 在帳號 B 上 `running`，Prompt 帶著檢查點內容，並說明沒有最後一輪摘要（L262-263）。
    - 沒有 `run_failed`，三種預算都是 0。
  - `sudden_limit.rs:274-307` 涵蓋另一種情況：CLI 撞到上限但不結束（在等額度）。Runtime 立刻停止它，不受 10 分鐘冷卻限制，由帳號 C 接手。
  - 實作位置：
    - `handoff.rs` 的 `candidates`：遇到 `limit_reached` 就略過冷卻。
    - `at_safe_boundary`：突然限流直接回傳 true，不等 checkpoint。
    - `supervisor.rs:1180-1189`：`Exited{success:false}` 時呼叫 `request_sudden_handoff`。
- [PASS] 全部受限時工作保存並等待，UI 顯示預計重評時間。證據如下：
  - `sudden_limit.rs:312-363`：舊 Run 以 `handed_off_waiting_resource` 結束，Task 進入 `waiting_resource`，checkpoint 有保存。`next_check_at` 等於最早的重置時間（30 分）再加 1 分鐘；31 分前一秒不派工，到 31 分時同一筆交接轉成 `transferred`。
  - `sudden_limit.rs:366-408`：重置時間未知時，退避從 5 分鐘開始，再來是 10 分鐘。Runtime 重啟後等待狀態與排定時間都還在。
  - Domain 測試 `without_a_reset_time_the_check_backs_off_from_five_to_sixty_minutes` 驗證退避上限是 60 分鐘。
  - UI：`web/src/TaskBoard.tsx` 的 `WaitingResourceSummary` 顯示「預計 … 重新評估」、依據的重置時間或退避次數，以及等待原因。`web/src/TaskBoard.test.tsx` 新增 4 個測試，都通過。
- [PASS] 資源恢復不會解除完全退出。證據如下：
  - `sudden_limit.rs:437-482`：完全退出後，即使 B 恢復且已過重評時間，`spawn_count` 仍是 1，交接停在 `waiting_resource`，`stop_intent_active=true`。重啟後在使用者按下「繼續工作」前也不派工；按下後才由 B 接手。
  - 實作在 `handoffs.rs` 的 `resume_resource_waits`：遇到 `halted(&reasons)`（Lifecycle 類別）就 `continue`，不派工也不重排。`request_handoffs` 在停止意圖生效時也不建立突然限流的交接。
- [PASS] 情境測試以 Runtime harness（D-04）撰寫並通過，AI 決策已追加。證據如下：
  - `sudden_limit.rs` 共 6 個測試，全部 ok。它用的是真實的 Axum API、SQLite 與 Supervisor，只換掉 Adapter、Clock、QuotaObserver 和 ProcessControl。
  - `ai-decisions.md` 已追加 AI-312～AI-318，每條都標了重要度。

TESTS:
- `pnpm test`：結束碼 0。
  - Rust：全部通過（`sudden_limit` 6/6、`handoff` 15/15、`retry_budgets` 6/6、baton_domain lib 532/532 等）。
  - 前端：vitest 20 個檔案、457 個測試全部通過。
- `pnpm build`：結束碼 0，產出 `target/release/baton-desktop.exe`。有一個警告：`baton-api/src/shared_settings.rs:20` 有未使用的 import `StoreError`，這個檔案不在本票 diff 內。
- `git status`：驗證前後都是乾淨的，沒有修改任何追蹤中的檔案。

前置條件：
- 驗證開始時，`rebuild/v1`（27722be）是 HEAD 的祖先，通過。
- 驗證途中整合分支前進到 496d291（已合併票 32），現在已不是 HEAD 的祖先。依指示只標註，不因此判 FAIL。
- 合併時要注意：票 32 也把它的 `guardian_journal` 列為第 31 個 migration，並使用 AI-312～AI-318。這和本票的 `handoff_waiting_resource`（第 31 個）及 AI-312～AI-318 衝突。兩者沒有共用資料表，但 `crates/baton-store/src/lib.rs` 的 `MIGRATIONS`、`supervisor.rs` 和 `docs/development.md` 都可能有文字衝突。

FIXES_NEEDED: none

合併前建議做整合（不影響判定）：把最新的 `rebuild/v1` 合併進本票分支，把 migration 順延為第 32 個，AI 編號改到 AI-319 之後。然後重新執行 `pnpm test` 和 `pnpm build`。
