票 53 驗證通過。六條驗收條件全部 PASS。用真實 Chromium 實跑 `node scripts/test-e2e.mjs`，3 個測試全過；正式建置也成功，工作樹全程乾淨。

前置條件：一開始 `rebuild/v1` 是 HEAD 的祖先，合併基準 34dd6fd。驗證途中整合分支因票 54 合併而前進到 4d20c91，所以現在祖先關係不成立。`git merge-tree HEAD rebuild/v1` 確認兩邊可以無衝突合併。依指示只標註，不影響判定。

```
VERDICT: PASS
CRITERIA:
- [PASS] 有一個只供測試使用的 Runtime 組態，以 fake Port 啟動真實 Runtime 與 API；正式建置與 NSIS 安裝程式不包含它 — 組態只在 `baton-harness` 的測試目標 `crates/baton-harness/tests/ui_e2e/`：`main.rs:44-58` 以 `Harness` 啟動，`server.rs:65-107` 是轉送伺服器，帶上桌面憑證與 `CLIENT_HEADER`、不帶 Origin，Runtime 的 API 防護沒有放寬。`tests/test_only_config.rs` 的 3 個測試通過：以 cargo metadata 證明安裝程式打包的程式到不了 harness、harness 只有 lib／test 目標、bundle 只有 externalBin、web/src 不引用測試組態。另外自己用 `cargo tree -e normal,build` 查了 baton-runtime、baton-guardian、baton-desktop，對 baton-harness 都是 0 筆。唯一引用 harness 的 `crates/baton-client/Cargo.toml:21` 在 `[dev-dependencies]`。`pnpm build` 成功。
- [PASS] UI E2E「派工到完成」 — `dispatch_to_completion.rs:30-128`：在 UI 建立 Workspace、套用並確認「研究／文件」授權（看到 revision 1）、建立 Goal 與 Task（填齊六項契約，宣告檢查 `artifact: report.md`）、標記可派工並派工。fake Worker 以 Agent 憑證 `report_progress` 與 `register_artifact`，然後 `Exited{success:true}`。UI 的「已完成（1）」欄出現這張卡片，顯示「已通過驗證（自動檢查）」與「Run（第 1 次）：成功」。API 佐證 task=completed、驗證方式是 checks／passed、Run 的授權 revision=1。實跑結果 `... ok`。
- [PASS] UI E2E「裁定中心批次處理」 — `decision_center_batch.rs:218-370`：3 個執行中的 Task 產生 4 筆請求（2 筆權限提示、1 筆 request_decision、1 筆 controller_action），在裁定中心逐項勾選，各自選擇方案後送出批次。UI 顯示每筆「已保存」；`decision` 表剛好 4 筆，共用同一個 batch_id。3 個 Task 都恢復為 ready，重新派工後在 UI 的「執行中（3）」欄。新 Run 證明每項批准只放行自己的那一次：再做一次、不同的操作、Controller 第二次都要重新裁定，UI 也顯示「使用，已失效」。最後一段用 `nth` 定位，我核對過：重複請求的問題文字不同（`decision.rs:1304-1307`），所以定位正確。實跑結果 `... ok`。
- [PASS] 沒有安裝 Playwright 時明確標為略過，文件說明如何執行 — 不設定 `BATON_PLAYWRIGHT_DIR` 跑 `pnpm test`：cargo 把 3 個 E2E 列為 ignored，結果列印出「UI E2E：略過（找不到 Playwright；…）」，結束碼 0。沒有 Playwright 時直接用 `--ignored` 執行，會在 `browser.rs:112` panic，顯示 FAILED，不會顯示通過。`scripts/test-e2e.mjs:44-48`：有設定 `BATON_PLAYWRIGHT_DIR` 卻找不到時失敗。執行方式寫在 `docs/development.md` 的〈UI E2E〉一節。
- [PASS] E2E 結束後不遺留暫存資料目錄或程序 — 腳本在專用 TEMP 執行，結束後檢查遺留（`test-e2e.mjs:69-74`），沒有報錯。`browser.rs:174-188` 斷言 browserExited=true，且 driver 結束碼是 0。我在執行前後各拍了一次程序與 %TEMP% 快照：沒有新增的 chrome／headless_shell／node／baton 程序；%TEMP% 除了我自己的紀錄檔沒有新項目，`baton-e2e-*` 已刪除。
- [PASS] AI 決策已追加至 ai-decisions.md — `ai-decisions.md:4928`（AI-503）、`:4941`（AI-504）、`:4954`（AI-505），都有標重要度。
TESTS:
- `pnpm test`（不設定 BATON_PLAYWRIGHT_DIR）→ 結束碼 0。Rust 通過，所有 test result 都是 ok，0 failed；前端 vitest 30 個檔案、648 個測試通過；UI E2E 列為略過。
- `node scripts/test-e2e.mjs`（BATON_PLAYWRIGHT_DIR=C:\Users\User\AppData\Local\Temp\baton-pw）→ 結束碼 0。`test result: ok. 3 passed; 0 failed`，最後印出「=== UI E2E：通過 ===」。
- `cargo test -p baton-harness --test ui_e2e -- --ignored the_web_ui_reaches`（沒有 Playwright）→ 依預期 FAILED，訊息是「UI E2E 無法開啟瀏覽器…」。
- `pnpm build` → 結束碼 0，產出 target\release\baton-desktop.exe。
- `cargo tree -p {baton-runtime,baton-guardian,baton-desktop} -e normal,build`，搜尋 baton-harness → 都是 0 筆。
- `git status --short` → 全程乾淨。
FIXES_NEEDED: none
```

有兩點不影響判定：
- CI 設定沒有改，所以 CI 只會把 UI E2E 列為略過，實際跑需要設定 `BATON_PLAYWRIGHT_DIR`。實作者已在 AI-504 說明這點。
- 本票兩條 E2E 測的是瀏覽器模式的 HTTP 用戶端，沒有測 Tauri 指令層。AI-503 已記錄這項取捨。
