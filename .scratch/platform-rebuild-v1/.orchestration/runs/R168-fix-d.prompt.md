你是 platform-rebuild-v1 最後的收尾修正 Worker（run R168-fix-d），分支 `rebuild/final-fix-d`（從 `rebuild/v1` 建立）。

實作規則、執行方式限制與回覆格式完全依照 `.scratch/platform-rebuild-v1/.orchestration/final-fix.template.md`（分支代入 `rebuild/final-fix-d`，範圍為整個 repo），只處理下列項目：

1. `review-notes.md` 第 29 項：
   - Web 的確認授權呼叫帶上 `expected_revision`（base revision），讓後端的並行編輯保護真的生效（AI-471）；補前端測試，以及一個 harness 情境測試證明兩個視窗並行編輯時後送出者會被拒絕。
   - Controller 重現案例的新限制（R151 第 3 項）寫進 MCP shim 的工具說明，並同步工具清單測試。
2. `review-notes.md` 第 30 項：清掉 `cargo clippy --workspace --all-targets -- -D warnings` 的錯誤（修程式碼，不要用 `#[allow]` 大範圍關掉；個別有正當理由的 allow 要附繁體中文註解），並把 clippy 加進 `pnpm test`（`scripts/test.mjs`）或 CI，讓之後不會再累積。
3. 順手清掉 `crates/baton-api/src/shared_settings.rs` 未使用的 `StoreError` import，以及 `crates/baton-harness/tests/gate_invalidation.rs` 未使用的 `error_code`（review-notes #23）。

真實瀏覽器測試沿用 `BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw`；請用專用 TEMP 跑測試並確認沒有殘留。
