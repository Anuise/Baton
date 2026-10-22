你是 platform-rebuild-v1 的驗證者（run R105-16-verify），負責以唯讀方式驗證第 16 張票。

工作目錄是這張票的 git worktree，分支 `rebuild/16-controller-conversation`。實作者的自述不是證據，請自行核對。

先讀：
- `AGENTS.md`
- `.scratch/platform-rebuild-v1/spec.md`（與本票相關的段落）
- 票：`.scratch/platform-rebuild-v1/issues/16-controller-conversation.md`
- `git diff rebuild/v1...HEAD`

要做的事：
1. 確認 `rebuild/v1` 是 HEAD 的祖先（實作者已合併整合分支）。
2. 逐條核對驗收條件，每條給出 PASS/FAIL 與證據（檔案:行號、指令輸出）。
3. 實際執行專案的建置與全部測試指令。
4. 檢查有沒有違反 spec 或明顯的缺陷。

唯讀：不要修改、新增或 commit 任何追蹤中的檔案；建置產生的忽略檔案可以存在。

**執行方式限制：** 這是 `claude -p` 非互動 run，你一結束回合，run 就結束。不要使用 Monitor、`run_in_background`、排程或任何「等通知再繼續」的方式；建置與測試一律在前景執行到結束（必要時把 Bash 的 timeout 設到 600000）。

最後一則回覆請用以下格式：
```
VERDICT: PASS | FAIL
CRITERIA:
- [PASS|FAIL] <驗收條件> — <證據>
TESTS: <指令與結果>
FIXES_NEEDED: <FAIL 時，給修正 run 的具體清單；PASS 時寫 none>
```

**補充：** 這是修正後的重新驗證（前次報告 `.scratch/platform-rebuild-v1/.orchestration/runs/R098-16-verify.result.md`）。若驗證途中整合分支前進，前置條件不成立時請標註但不要因此判 FAIL，只依功能驗收條件判定。
