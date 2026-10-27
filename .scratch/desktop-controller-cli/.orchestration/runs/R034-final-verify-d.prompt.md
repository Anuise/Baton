你是 desktop-controller-cli 最終修正的驗證者（run R034-final-verify-d），以唯讀方式驗證分支 `cli/final-d` 的修正。

工作目錄是該分支的 worktree。實作者的自述不是證據，請自行核對。

先讀：
- 本範圍沒有審查報告；要驗證的是 `review-notes.md` 第 11、12 項
- 修正 run 的回報：`C:/Users/User/Documents/au-program/issue-orchestrator/.scratch/desktop-controller-cli/.orchestration/runs/R032-fix-d.result.md`
- `.scratch/desktop-controller-cli/.orchestration/review-notes.md`
- `git diff cli/v1...HEAD`

要做的事：
1. 逐項核對 `review-notes.md` 第 11、12 項與修正回報的 FIXED（第 11 項請照回報的延遲注入步驟，確認修正前會失敗、修正後通過）：每項是否真的修好、是否有會在修正前失敗的測試。抽查至少全部「高」與「中」項目。
2. 核對 NOT_FIXED 的理由是否合理、是否都有對應的 AI 決策；不合理的列為需修正。
3. 檢查修正有沒有引入新缺陷、有沒有刪除或弱化既有測試。
4. 實際執行 `pnpm test`、`pnpm --filter web typecheck`、`cargo check -p baton-desktop`。

唯讀：不要 commit，也不要留下對追蹤中檔案的修改。為了延遲注入可以暫時修改測試檔，但跑完一定要以 `git checkout -- <檔案>` 還原，結束時 `git status --short` 必須是空的。若驗證途中整合分支前進，前置條件不成立時請標註但不要因此判 FAIL。

**執行方式限制：** 這是 `claude -p` 非互動 run，你一結束回合，run 就結束。不要使用 Monitor、`run_in_background` 或任何「等通知再繼續」的方式；建置與測試一律在前景執行到結束。

最後一則回覆請用以下格式：
```
VERDICT: PASS | FAIL
CHECKED:
- [PASS|FAIL] <項目> — <證據>
TESTS: <指令與結果>
FIXES_NEEDED: <FAIL 時，給修正 run 的具體清單；PASS 時寫 none>
```
