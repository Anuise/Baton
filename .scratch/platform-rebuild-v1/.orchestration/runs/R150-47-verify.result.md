建置與全部測試都通過，上次指出的缺口也確認已修好，最後整理結論如下。

```
VERDICT: PASS
CRITERIA:
- [PASS] 前置條件：rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 成立（rebuild/v1 = fa3a55d，HEAD = fefcd15 的合併 commit 已含它）。
- [PASS] 環境故障不被記為產品缺陷 — 修正 commit c03aff8 沒有動到判定邏輯。只有「案例以產品行為斷言失敗，而且對照的代表旅程全部通過」才會升為 confirmed，其他情況都是 Inconclusive、狀態不變。情境測試 `an_environment_failure_is_never_recorded_as_a_product_defect` 本次執行 ok。
- [PASS] 未經重現的疑點不會顯示為 confirmed — 回報一律是 suspected；資料庫 CHECK 要求「缺陷狀態 ⇔ 有重現案例」；UI 把已確認缺陷、疑點、未驗風險分區顯示。測試 `a_suspicion_is_only_shown_as_confirmed_after_a_reproduction_fails_on_a_product_assertion` 本次執行 ok。
- [PASS] 不能以排除標籤或刪除案例掩蓋缺陷 — R148 指出的缺口已修好：
  - Store 在分流的寫入交易內先檢查有沒有排隊中的重現，有就拒絕（crates/baton-store/src/exploration.rs:162-172 的 `reproduction_pending`，呼叫點 :497-500），回 409 `exploration_reproduction_pending`（domain exploration.rs:83）。
  - API 在送裁定之前也檢查一次（crates/baton-api/src/exploration.rs:236-239），所以 expected_design 不會先建立裁定請求。
  - `is_diagnostic()` 涵蓋 Reproduce 和 VerifyFix（domain :562-564），與 Store 查詢的 purpose 一致。
  - 反方向也擋住了：已排除的疑點不能排入重現，要先 reopen（domain :701-704）。
  - 新增的情境測試 `a_suspicion_cannot_be_triaged_away_while_its_reproduction_is_queued`（crates/baton-harness/tests/exploration_to_reproduction.rs:498-526）：environment、expected_design、mark_unverified 三種分流都回 409，沒有建立「預期設計」裁定，supervise 之後疑點成為 confirmed、沒有 exclusion。本次執行 ok。
  - 原本的 `a_defect_cannot_be_hidden_by_exclusion_labels_or_by_dropping_its_case` 也 ok。
- [PASS] 情境測試以主要 seam（Runtime harness，spec D-04）撰寫、涵蓋本票行為並通過；AI 決策已追加至 ai-decisions.md — exploration_to_reproduction.rs 的 5 個測試全部 ok。ai-decisions.md 新增 AI-446（重要度：中，記錄本次修正與替代方案）與 AI-447（重要度：低，整合票 48 時重新編號）。
TESTS:
- `pnpm build`（BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw）→ exit 0，產出 target/release/baton-desktop.exe。唯一的警告是 crates/baton-api/src/shared_settings.rs:20 有 `StoreError` 沒用到；這個檔案不在本票的 diff 內。
- `pnpm test`（同一個 env）→ exit 0。
  - Rust 合計 1657 passed、0 failed、8 ignored。8 個 ignored 都是需要已登入 Claude Code 的 live 測試，與本票無關。
  - 真實瀏覽器測試有實際執行而且通過：`web_verification_real_browser` 的 `a_real_browser_logs_in_by_secret_reference_and_keeps_only_safe_evidence`、`a_session_drives_a_real_browser_and_returns_its_trace_and_video`。
  - 前端 vitest：30 個測試檔、636 個測試全部通過。
- 驗證結束時 `git status` 是乾淨的，沒有修改任何追蹤中的檔案。
FIXES_NEEDED: none
```
