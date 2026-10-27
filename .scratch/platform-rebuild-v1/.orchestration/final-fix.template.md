你是 platform-rebuild-v1 最終 code-review 的修正 Worker（run {{RUN}}），負責修正下列範圍內的問題：

**範圍：** {{SCOPE}}

工作目錄是專用 worktree，分支 `{{BRANCH}}`，從整合分支 `rebuild/v1` 建立。另外有兩個修正 run 同時處理其他範圍，所以請**盡量只改本範圍內的檔案**；非改不可的跨範圍修改要在 NOTES 說明。

先讀：
- `AGENTS.md`、`.scratch/platform-rebuild-v1/execution.md`、`.scratch/platform-rebuild-v1/spec.md`、`.scratch/platform-rebuild-v1/ai-decisions.md`
- 本範圍的審查報告：`{{REPORT}}`（若不在 worktree 中，讀主 checkout 的同名路徑 `{{ORCH}}/runs/{{REPORT_FILE}}`）
- 已知待修清單：`.scratch/platform-rebuild-v1/.orchestration/review-notes.md`

要處理的項目：
1. 審查報告的全部 FINDINGS，依嚴重度由高到低處理。
2. `review-notes.md` 中，主要程式位置落在本範圍內的項目。

規則：
- 以 TDD 修正（有 `tdd` skill 就用它）：每項缺陷先寫出會失敗的測試，再修到通過。純文字或註解類的小問題可以直接改。
- 不刪除、不弱化既有測試。
- 某項判斷後決定不修（例如牽涉產品取捨、或修了風險更大），在 `ai-decisions.md` 追加條目說明理由，標「最終 review 修正」、`- 審閱：未審`，並在 NOTES 列出。
- 設計上的判斷一樣追加到 `ai-decisions.md`（編號接續最大號，不改既有條目）。
- 程式碼註解用繁體中文。
- 小步 commit 到 `{{BRANCH}}`，結尾加 `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`。
- 結束前 `git merge rebuild/v1`（若有新 commit），跑 `pnpm test`、`pnpm --filter web typecheck`、`cargo check -p baton-desktop`，全部通過，工作樹乾淨。不要 push，不要動 `main` 或 `rebuild/v1`。
- 真實瀏覽器測試請沿用 `BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw`，不要另外安裝。

**執行方式限制：** 這是 `claude -p` 非互動 run，你一結束回合，run 就結束。不要使用 Monitor、`run_in_background` 或任何「等通知再繼續」的方式；建置與測試一律在前景執行到結束。可以用 subagent 平行處理彼此獨立的項目（同時最多 3 個）。

最後一則回覆請用以下格式：
```
RESULT: DONE | FAILED
COMMITS: <第一個>..<最後一個>
TESTS: <跑了哪些指令與結果>
FIXED: <已修正的項目，每項一行：來源（R15x 第 n 項／review-notes #n）— 修法 — 測試>
NOT_FIXED: <未修正的項目與理由（對應的 AI-NN）>
NOTES: <跨範圍修改、需要驗證者特別看的地方>
```
