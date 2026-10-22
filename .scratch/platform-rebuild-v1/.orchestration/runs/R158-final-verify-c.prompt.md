你是 platform-rebuild-v1 最終修正的驗證者（run R158-final-verify-c），以唯讀方式驗證分支 `rebuild/final-fix-c` 的修正。

工作目錄是該分支的 worktree。實作者的自述不是證據，請自行核對。

先讀：
- 審查報告：`/c/Users/User/Documents/au-program/issue-orchestrator/.scratch/platform-rebuild-v1/.orchestration/runs/R153-review-c.result.md`
- 修正 run 的回報：`/c/Users/User/Documents/au-program/issue-orchestrator/.scratch/platform-rebuild-v1/.orchestration/runs/R156-fix-c.result.md`
- `.scratch/platform-rebuild-v1/.orchestration/review-notes.md`
- `git diff rebuild/v1...HEAD`

要做的事：
1. 逐項核對審查報告的 FINDINGS 與修正回報的 FIXED：每項是否真的修好、是否有會在修正前失敗的測試。抽查至少全部「高」與「中」項目。
2. 核對 NOT_FIXED 的理由是否合理、是否都有對應的 AI 決策；不合理的列為需修正。
3. 檢查修正有沒有引入新缺陷、有沒有刪除或弱化既有測試。
4. 實際執行 `pnpm test`、`pnpm --filter web typecheck`、`cargo check -p baton-desktop`。

唯讀：不要修改、新增或 commit 任何追蹤中的檔案。若驗證途中整合分支前進，前置條件不成立時請標註但不要因此判 FAIL。

**執行方式限制：** 這是 `claude -p` 非互動 run，你一結束回合，run 就結束。不要使用 Monitor、`run_in_background` 或任何「等通知再繼續」的方式；建置與測試一律在前景執行到結束。

最後一則回覆請用以下格式：
```
VERDICT: PASS | FAIL
CHECKED:
- [PASS|FAIL] <項目> — <證據>
TESTS: <指令與結果>
FIXES_NEEDED: <FAIL 時，給修正 run 的具體清單；PASS 時寫 none>
```

**補充：** 也請確認 review-notes #28（harness 測試遺留暫存資料目錄）是否已修正：跑完 harness 測試後 `%TEMP%` 不應新增殘留的 Baton 資料目錄。真實瀏覽器測試沿用 `BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Tempaton-pw`。
