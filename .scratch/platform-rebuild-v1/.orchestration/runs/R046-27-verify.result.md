票 27 驗證通過。四條驗收條件都成立，`pnpm test` 和 `pnpm build` 都成功，工作樹沒有任何改動。另外找到一個與 AI-124 不一致的小問題，不影響驗收條件，寫在最後。

```
VERDICT: PASS
CRITERIA:
- [PASS] 前置：rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 回傳 0。
- [PASS] 觀測不到時顯示「未知」而非 0 — 各層都把未知存成 None/null，不填 0：
  - Domain：`assess` 沒有任何觀測時 `used=None`、`confidence=Unknown`（crates/baton-domain/src/quota.rs:381-470）。沒有方案上限、或視窗內沒有本機用量時，`estimate` 回 None（quota.rs:321）。CLI 訊號沒有數字、狀態也不是 exhausted 時，可信度是 Unknown（quota.rs:296）。
  - 資料表：migration 13 未知值存 NULL（crates/baton-store/src/lib.rs）。
  - 排程：`known_used` 只放已知用量的帳號（crates/baton-api/src/quota.rs:79-82）。
  - UI：`quotaLabel` 遇到 null 顯示「未知」（web/src/AccountsPage.tsx `quotaLabel`）。
  - 情境測試 4 條全過：an_account_without_any_observation_is_unknown_not_zero、usage_without_a_plan_limit_is_still_unknown、an_exhausted_signal_counts_until_its_reset_then_returns_to_unknown（重置後回到 null）、a_failing_quota_observer_leaves_an_unknown_observation_not_zero。前端測試「完全未知時判斷用量顯示「未知」而非 0%」通過。
- [PASS] 已觀測與推估並存時以較保守者判斷 — 每個視窗取兩個來源中用量較高者，同值時選已觀測；帳號再取兩個視窗與外部觀測器中最高者（quota.rs:367-379、393-416）。`quota_used` 同時拿來做派工判斷：Supervisor 經 `quota::current` → `known_used` 進入 `schedule`（crates/baton-runtime/src/supervisor.rs），派工 API 也走這條路（crates/baton-api/src/scheduling.rs `known_quota`）。
  - 情境測試通過：observed_and_estimated_coexist_and_the_more_conservative_one_decides_dispatch（已觀測 30%、推估 80%，判斷值 0.8，派工回 202、原因 quota）；a_higher_observation_overrides_a_lower_estimate（已觀測 0.9 蓋過推估 0.2）。
  - Domain 測試 the_more_conservative_of_observed_and_estimated_wins_in_either_direction 通過。
- [PASS] 其他裝置用量不可見時明確標示觀測不完整 — 任一視窗缺少已觀測用量時 `complete=false`，`incomplete_reason` 註明「其他裝置與互動式使用的用量看不到」（quota.rs:427-443）。UI 以 `role="note"` 的 `.quota-incomplete` 顯示這段說明。
  - 情境測試 estimates_alone_are_marked_incomplete_because_other_devices_are_invisible 通過：只有推估時不完整；只有五小時有已觀測時仍不完整，原因含「七日」；兩個視窗都有已觀測才完整。
  - 前端測試「觀測不完整時以提示說明原因；完整時不顯示提示」通過。
- [PASS] 以 Runtime harness（D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加 —
  - crates/baton-harness/tests/quota.rs 共 10 條，經真實 Axum API、scripted fake AgentAdapter、fake Clock、fake QuotaObserver、真實 SQLite，10 passed。
  - 涵蓋範圍包含原始資料參照（raw_ref 為 `run:<id>`、`quota_usage:seq=…`，原文有落地、不經 API 回傳）、D-31 每 5 分鐘輪詢（5 分鐘少 1 毫秒時不輪詢，滿 5 分鐘才輪詢）、方案上限的驗證與重啟後保留。
  - ai-decisions.md 已追加 AI-121～AI-127，都標了重要度。
  - 帳號頁每 5 分鐘重新讀取，前端測試「每 5 分鐘重新讀取排程總覽」「離開帳號頁後不再輪詢」通過。
TESTS:
- `pnpm test`（= cargo test --workspace --exclude baton-desktop + pnpm --filter web test）→ EXIT=0
  - Rust：全部 test result ok，0 failed。其中 baton-domain 235 passed（含 quota::tests 20 條）、harness tests/quota.rs 10 passed、baton-client 含 app_sets_plan_limits_and_reads_the_quota_assessment。
  - 前端：Test Files 13 passed、Tests 205 passed。
- `pnpm build` → EXIT=0，release 版 baton-desktop.exe 建置成功。
- `git status --short` 為空，沒有改動任何追蹤中的檔案。
FIXES_NEEDED: none
```

**不擋本票的問題：** AI-124 寫的是只有「帶數字（或 exhausted）」的已觀測訊號才算觀測完整，但程式碼沒有照做。

- **位置：** crates/baton-domain/src/quota.rs:427-433 用 `judged_used().is_none()` 判斷某個視窗是否缺少已觀測用量。
- **後果：** 沒有數字的 `warning` 訊號，`judged_used()` 會回 `Some(0.75)`，所以不會被算成缺少。若兩個視窗都只有這種警告，`complete=true`，帳號頁就不顯示「觀測不完整」，但實際用量只知道一個下限。
- **測試：** 目前沒有測試涵蓋這個情況。
- **建議修法：** 把判斷改成「`used.is_some()` 或 `status == Exhausted`」，並補一條 Domain 測試。可以留給後續的票（例如票 28）一起處理。
