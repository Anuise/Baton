你是 platform-rebuild-v1 的整合 Worker（run R135-44-integrate），負責讓第 44 張票的分支 `rebuild/44-two-axis-review` 與最新的整合分支 `rebuild/v1` 合併。

背景：本票與另一張平行開發的票改到了同一批檔案；那張票已先合併進 `rebuild/v1`。本票本身已實作完成，這個 run 只處理整合，不新增功能。

先讀 `AGENTS.md`、`.scratch/platform-rebuild-v1/execution.md`、票 `.scratch/platform-rebuild-v1/issues/44-two-axis-review.md`，以及 `git log --oneline HEAD..rebuild/v1` 與 `git diff HEAD...rebuild/v1` 了解整合分支新增了什麼。

要做的事：
1. `git merge rebuild/v1`，逐一解決衝突，保留兩邊的行為。
2. Store migration 編號若與整合分支重複，把本票的 migration 順延到整合分支最大編號之後，並確認兩邊的 schema 都會被套用。
3. `ai-decisions.md` 中本票新增的 AI-NN 若與整合分支重號，順延到整合分支最大編號之後，並同步更新本票程式碼、文件裡對這些編號的引用。不能修改整合分支已有的條目。每個條目都要有 `- 審閱：` 行。
4. 跑 `pnpm test`（必要時 `pnpm --filter web typecheck`、`cargo check -p baton-desktop`），全部通過；本票與整合分支的測試都不可刪除或弱化。
5. commit 到 `rebuild/44-two-axis-review`，結尾加 `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`。結束時 `rebuild/v1` 必須是 HEAD 的祖先、工作樹乾淨。不要 push，不要動 `main` 或 `rebuild/v1`。
6. 若解衝突時需要做判斷，依 `ai-decisions.md` 格式追加新條目（標「票 44 整合」，`- 審閱：未審`）。

**執行方式限制：** 這是 `claude -p` 非互動 run，你一結束回合，run 就結束。不要使用 Monitor、`run_in_background`、排程或任何「等通知再繼續」的方式；建置與測試一律在前景執行到結束（必要時把 Bash 的 timeout 設到 600000）。

最後一則回覆請用以下格式：
```
RESULT: DONE | FAILED
COMMITS: <第一個>..<最後一個>
TESTS: <跑了哪些指令與結果>
NOTES: <衝突怎麼解、改了哪些編號>
```

**補充：** 驗證報告 `.scratch/platform-rebuild-v1/.orchestration/runs/R133-44-verify.result.md` 已列出衝突檔案、編號與 mcp-shim 工具數斷言的更新要求，請一併處理。
