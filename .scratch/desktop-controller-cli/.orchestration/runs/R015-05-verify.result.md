VERDICT: FAIL

整體結論：功能本身都做到了，建置、Clippy、Rust 測試與 UI E2E 也都通過。不通過的原因只有一個：本票改動後，前端一個既有測試變得不穩定，`pnpm test` 會間歇性失敗。在相同條件下重跑整套前端測試，`cli/v1` 跑 12 次全過；本票分支 17 次裡失敗 5 次，所以判定是本票引入的。

先確認了祖先關係：`git merge-base --is-ancestor cli/v1 HEAD` 成功，輸出 `ANCESTOR_OK`。

CRITERIA:
- [PASS] Controller Run 非終態期間，Scheduler 不把該 Profile 分給新的 Worker；已在執行的 Worker 繼續執行；Run 結束後該 Profile 恢復為候選 — 證據如下：
  - 排程判斷：`crates/baton-domain/src/scheduler.rs:403-411` 會跳過這類帳號，並給出 `controller_account` 等待原因。
  - 帳號清單來源：`crates/baton-store/src/controller_cli.rs:1056` 的 `load_controller_accounts` 只抓狀態為 `starting`、`running` 的 Run。
  - 套用範圍：這份清單用在 `scheduling.rs` 的三個 evaluate 呼叫點，以及 `handoffs.rs:614` 的交接承接者。全專案搜尋過所有排程判斷的呼叫點，派 Worker 的路徑沒有遺漏。只有 supervisor.rs 裡兩處舊的 `claude -p` Controller 路徑傳空清單，那是待移除的舊機制。
  - 情境測試：`controller_cli_accounts.rs:175` 測試 `while_the_controller_cli_is_open_its_account_gets_no_new_workers_until_it_ends`，涵蓋「新工作排隊、既有 Worker 沒被停止且仍在執行、結束後佇列派出」。另有 `:217`，以及 domain 層的 3 個 scheduler 測試。
- [PASS] Controller 頁簽與 Profile 選單顯示額度與執行中 Worker 數；達到 draining 門檻時顯示提示 — 證據如下：
  - 選單：`web/src/ControllerPage.tsx` 的 `quotaSummary`、`optionLabel` 用在開啟選單與換帳號選單。
  - 頁簽：開著時以 `controller-quota` 區塊顯示 Controller 帳號的額度。
  - 提示：`quota_gate.state === "draining"` 時顯示 `role="status"` 提示。
  - 元件測試：`ControllerPage.test.tsx` 新增的測試涵蓋額度、未知額度、輪詢更新與 draining 提示。
- [PASS] 同一 Workspace 以同一 Profile 重開時啟動規格含 `--continue`；換 Profile 時不含 — 證據如下：
  - 判斷規則：`baton-domain/src/controller_cli.rs:180` 的 `continues_session`。
  - 加參數：`claude_adapter.rs:132-135` 依此加上 `--continue`。
  - 原生 Session 檔不存在時改開新的 Session：`begin_launch` 帶 `continuable` 參數，搭配 `continue_capability`。
  - 情境測試：`controller_cli_accounts.rs:237`，使用真的 adapter，涵蓋同帳號重開、換帳號、跨 Workspace 三種情況。
- [PASS] 「換帳號重開」先結束目前 Controller Run 再開新的，兩者在時間線上可追查 — 證據如下：
  - 流程：`request_switch` 先記下新 Profile 與結束要求；結束時 `end_cli`（`store/controller_cli.rs:1016`）在同一個交易中呼叫 `open_switched` 開新的 Run。
  - 時間線：harness 測試 `:278` 斷言的事件順序是 `switch_requested` → `ended(end_code=switched)` → `opened(switched_from_run_id)` → `launch_started` → `running`，並確認舊憑證失效（401）。
  - 另外兩個情境：`:342` 是使用者先自己關掉終端機，仍會開出新的 Run；`:370` 是換成同一個或不可用的帳號會被拒。
- [FAIL] 以 Runtime harness 撰寫情境測試涵蓋上述行為並通過；Controller 頁簽元件測試涵蓋額度與換帳號 — 證據如下：
  - harness 6 個情境都通過，新增的元件測試也涵蓋了額度與換帳號。
  - 但同一個 `web/src/ControllerPage.test.tsx` 裡，既有測試「開啟中每 2 秒重新讀取，變成執行中，卸載後停止」在本分支會間歇性失敗（`:221`）。錯誤是 `expected "vi.fn()" to be called 2 times, but got 1 times`。
  - 這個測試單獨跑 10 次都沒失敗，只在整套平行執行時出現，`cli/v1` 上沒有出現過。
  - 合理的推測：本票新增的 `schedulingOverview` 讀取多了一次 state 更新，讓輪詢計時器的建立時間在假計時器下變得不固定。
- [PASS] AI 決策已追加至 ai-decisions.md — AI-23～AI-26 已追加，各自標了重要度（中、中、中、低）。

TESTS:
- `node scripts/test.mjs`：Clippy 通過、Rust 通過、UI E2E 通過；前端失敗（結束碼 1，683 個測試中 1 個失敗，就是上面那個測試）。
- `pnpm --filter web test` 再跑 3 次：通過、失敗 1 個、通過。
- 只跑 `npx vitest run src/ControllerPage.test.tsx` 10 次：0 次失敗。
- 整套前端測試（`npx vitest run`）比較：
  - 本分支：再跑 12 次失敗 4 次，加上前面共 17 次失敗 5 次，失敗的都是同一個測試。
  - `cli/v1`：用 `git archive` 匯出的暫存副本跑 12 次，682 個測試全部通過。
- `node scripts/build.mjs`：EXIT=0，產出 `target/release/baton-desktop.exe`。
- 驗證結束後 `git status` 乾淨，暫存副本已刪除。

FIXES_NEEDED:
1. 修正 `web/src/ControllerPage.test.tsx` 的「開啟中每 2 秒重新讀取，變成執行中，卸載後停止」（約第 208-226 行），讓它在整套平行執行下穩定通過：
   - 推進假計時器之前，先等初次載入全部完成並讓 effect 執行完。例如在 `findByText("開啟中")` 之後加上 `await waitFor(() => expect(client.schedulingOverview).toHaveBeenCalledTimes(1))` 與 `await act(async () => {})`，確保輪詢計時器已經建立。
   - 或改用 `await act(async () => vi.advanceTimersByTime(...))`、`await vi.waitFor(...)` 來斷言呼叫次數。
   - 如果確認是元件問題，也可以在 `ControllerPage.tsx` 讓輪詢計時器的建立不受 overview 讀取時機影響。
   - 不要放寬斷言。
2. 修正後在 `web/` 連續跑 `npx vitest run` 至少 10 次全部通過，再跑一次 `node scripts/test.mjs` 確認四項都通過。
exit: 0


---

主控補充（2026-10-23，派修正 run 前）：

- 驗證之後 `cli/v1` 已合併票 07（完全退出與恢復）與票 06（事件等待）。修正前先 `git merge cli/v1`；目前衝突的檔案有 `crates/baton-domain/src/controller_cli.rs`（含 `tests.rs`）、`crates/baton-ports/src/agent.rs`、`crates/baton-runtime/src/supervisor/controller_cli.rs`、`crates/baton-store/src/controller_cli.rs`、`crates/baton-store/src/lib.rs`、`web/src/ControllerPage.tsx` 等。
- **`--continue` 只能有一份實作。** 票 07 已在 `cli/v1` 以 migration `controller_cli_recovery` 加了 `continue_session` 欄位與「這個 Workspace 最近一次真的執行過的 Run 用同一個帳號」的規則（cli/v1 的 AI-25）。本票的 migration `controller_cli_switch` 不可再加 `continue_session`，只保留本票自己需要的欄位（如 `switch_to_profile_id`）；判斷沿用票 07 的欄位與規則，再加上本票的原生 Session 查驗（`continue_capability`，Session 檔確定不存在就不帶 `--continue`）。兩票的 `--continue` 情境測試都要通過。
- FIXES_NEEDED 第 1 項的不穩定測試，票 07 已在 `cli/v1` 修過同一個測試（commit 9117308：推進計時器前先 `await act(async () => {})`）。合併後若仍不穩定，再依第 1 項處理；第 2 項的 10 次連跑仍要做。
- 換帳號重開與票 07 的「重啟後重新綁定（`rebound_at`）」、票 06 的事件游標（新 Run 接續上一個 Run 確認到的位置）要能共存：換帳號開出的新 Run 也要接續游標。
- 本票的 AI 條目順延到 `cli/v1` 最大號之後（目前 `cli/v1` 已到 AI-32；若票 08 先合併會更大，以合併當下為準）。
