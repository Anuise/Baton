你是 desktop-controller-cli 最終 code-review 的修正 Worker（run R032-fix-d），負責修正下列範圍內的問題：

**範圍：** `review-notes.md` 第 11 項（`web/src/VerificationPage.test.tsx` 在加壓下的不穩定測試）與第 12 項（`crates/baton-harness/tests/verification_handoff.rs`、`verification_takeover_blocked.rs` 檔頭說明過時）

工作目錄是專用 worktree，分支 `cli/final-d`，從整合分支 `cli/v1` 建立。另外有一個修正 run（Rust 後端）同時處理另一個範圍，所以請**盡量只改本範圍內的檔案**；非改不可的跨範圍修改要在 NOTES 說明。

先讀：
- `AGENTS.md`、`.scratch/desktop-controller-cli/execution.md`、`.scratch/desktop-controller-cli/spec.md`、`.scratch/desktop-controller-cli/ai-decisions.md`
- 背景：最終修正 C 的回報 `.scratch/desktop-controller-cli/.orchestration/runs/R030-fix-c.result.md` 與驗證 `R031-final-verify-c.result.md`（不在 worktree 中就讀 `C:/Users/User/Documents/au-program/issue-orchestrator/.scratch/desktop-controller-cli/.orchestration/runs/` 下的同名檔）；穩定前端測試的既有做法見 AI-52
- 已知待修清單：`.scratch/desktop-controller-cli/.orchestration/review-notes.md`

要處理的項目：
1. `review-notes.md` 第 11、12 項。第 11 項先以加壓或延遲注入重現失敗，再修到穩定；不要放寬斷言，也不要只靠拉長逾時。

規則：
- 以 TDD 修正（有 `tdd` skill 就用它）：每項缺陷先寫出會失敗的測試，再修到通過。純文字或註解類的小問題可以直接改。
- 不刪除、不弱化既有測試。
- 某項判斷後決定不修（例如牽涉產品取捨、或修了風險更大），在 `ai-decisions.md` 追加條目說明理由，標「最終 review 修正」、`- 審閱：未審`，並在 NOTES 列出。
- 設計上的判斷一樣追加到 `ai-decisions.md`（編號接續最大號，不改既有條目）。
- 程式碼註解用繁體中文。
- 小步 commit 到 `cli/final-d`，結尾加 `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`。
- 結束前 `git merge cli/v1`（若有新 commit），跑 `pnpm test`、`pnpm --filter web typecheck`、`cargo check -p baton-desktop`，全部通過，工作樹乾淨。不要 push，不要動 `main` 或 `cli/v1`。
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
