你是 desktop-controller-cli 的最終整體驗證者（run R035-final-check），以唯讀方式驗證整合分支 `cli/v1` 的最終狀態。

工作目錄是 `cli/v1` 最新 commit 的唯讀 worktree（detached HEAD）。九張票都已結案，最終 code-review 的三份修正（final-c、final-ab、final-d）也已各自驗證後合併；但 final-ab 與 final-d 是分別驗證的，合併後的組合還沒有整體跑過。實作者與先前驗證者的自述不是證據，請自行核對。

先讀：
- `AGENTS.md`
- `.scratch/desktop-controller-cli/spec.md`
- `.scratch/desktop-controller-cli/issues/`（九張票的驗收條件）
- `.scratch/desktop-controller-cli/.orchestration/review-notes.md`（已知待修清單，第 13、14 項尚未處理）
- `git log --oneline rebuild/v1..HEAD` 與 `git diff rebuild/v1...HEAD --stat`

要做的事：
1. 實際執行 `pnpm test`（設 `BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw`；若單次超過前景 600 秒上限，照 `scripts/test.mjs` 的內容拆段在前景跑完）、`pnpm --filter web typecheck`、`cargo check -p baton-desktop`、`pnpm build`。
2. 抽查跨票整合點在最終狀態仍成立，每項給出檔案:行號與對應測試：
   - Controller CLI 啟動規格：環境清理、`CLAUDE_CONFIG_DIR`、strict MCP、操作手冊檔（`--append-system-prompt-file`）、同帳號且原生 Session 存在時恰好一個 `--continue`、無 `-p`／權限模式。
   - Controller 憑證在 Run 結束、重新綁定（`rebound_at`）、換帳號後失效；Worker 憑證不能呼叫等待端點。
   - Controller 帳號排除：一般派工、佇列重試、交接承接都排除；結束後恢復。
   - 完全退出列出並結束 Controller CLI；崩潰恢復的存活／不存活處理；換帳號要求在當掉後仍會開新 Run。
   - 舊對話：沒有對話頁簽、沒有自動 Controller 輪次、資料表保留。
   - `.baton/ai-decisions.md` 範本建立、不覆寫。
3. 檢查 `.scratch/desktop-controller-cli/ai-decisions.md`：AI-01～AI-63 編號連續不重複，每個條目都有重要度與 `- 審閱：` 行。
4. 檢查工作樹與暫存：測試後沒有遺留 headless Chromium、`driver.mjs`、harness 資料目錄或測試程序。

唯讀：不要修改、新增或 commit 任何追蹤中的檔案；建置產生的忽略檔案可以存在。

**執行方式限制：** 這是 `claude -p` 非互動 run，你一結束回合，run 就結束。不要使用 Monitor、`run_in_background` 或任何「等通知再繼續」的方式；建置與測試一律在前景執行到結束（必要時把 Bash 的 timeout 設到 600000）。

最後一則回覆請用以下格式：
```
VERDICT: PASS | FAIL
CHECKED:
- [PASS|FAIL] <項目> — <證據>
TESTS: <指令與結果>
FIXES_NEEDED: <FAIL 時，給修正 run 的具體清單；PASS 時寫 none>
```
