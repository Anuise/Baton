我以唯讀方式驗證了票 09，結論是 **PASS**：四條驗收條件都成立，建置與全部測試都通過，Playwright 在本機可用，所以新的 UI E2E 旅程是真的執行並通過了。

```
VERDICT: PASS
CRITERIA:
- [PASS] 前置：cli/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor cli/v1 HEAD` 成功；cli/v1..HEAD 共 3 個 commit（d9e3c7b、d5b9b64、5f90284），diff 只動到 8 個檔案。
- [PASS] fake Port E2E 旅程通過；沒有安裝 Playwright 時明確標為略過 — 新旅程在 crates/baton-harness/tests/ui_e2e/controller_cli_open_and_end.rs:41-99，並在 main.rs:142 宣告。流程全部經畫面完成：側欄「選擇 beta」(:55) → Controller 頁簽 (:56) → 從「Controller 帳號」選單選第二個 Profile (:63) → 開啟 (:64) → 開啟中 (:65) → supervise 後顯示「執行中」(:69) → 結束 (:82) → 已結束 (:85)。另外斷言：fake 環境 Port 只開了一個視窗，CLAUDE_CONFIG_DIR 是選的帳號、工作目錄是該 Workspace (:73-77)；宿主已被 stop 且不再存活，API 回傳的 Run 是 ended (:90-95)。driver.mjs:41 新增 exactText，對應 browser.rs:41-43。實跑 `pnpm test`：`controller_cli_open_and_end::a_controller_cli_opened_from_the_controller_tab_runs_and_ends ... ok`（4 passed）。把 BATON_PLAYWRIGHT_DIR 設成空字串再跑 `node scripts/test-e2e.mjs`：印出「UI E2E 略過：找不到 Playwright…」與「=== UI E2E：略過（沒有執行，不代表通過） ===」，結束碼 0。測試標了 #[ignore]（:42）。
- [PASS] Windows 驗收清單新增上述手動項目，每項寫明步驟與預期結果 — docs/windows-acceptance.md 每項都是「步驟／預期結果」兩欄的表格：Windows Terminal／預設主控台開窗 A1、A8；原生 claude 互動 A4、A9、A10；Controller 工具可用 A5、A13（另有 A11 操作手冊、A12 決策紀錄、A14 權限）；背景 wait 收到 Worker 結束通知 B1-B6；Controller 帳號不被派工 C1-C4；完全退出關窗 D1-D3；以既有 .claude-Work-n 建 Profile E1-E5。我把清單中的預期字串和結束碼逐一對照程式碼，全部一致：「Baton 事件（N 件）：」「以上事件已確認，不會再回報」(controller_wait.rs:259-263)、「等待逾時：…」(wait.rs:77)、「Controller 憑證已失效…」(wait.rs:60)、「無法連線到 Runtime」(wait.rs:155)、結束碼 0/1/3/4 (controller_wait.rs:33-41)、C2 的排除原因 (scheduler.rs:407)、E2/E3 的錯誤訊息 (profile.rs:345-347)、操作手冊標題 (playbook/controller.md:1)。票 03 原本放在 docs/validation.md 的 7 項已整節搬成 A1-A7 與 F 節，內容沒有遺失；docs/development.md 也加了連結。
- [PASS] E2E 結束後不遺留暫存資料目錄或程序 — 檢查沿用既有機制：scripts/test-e2e.mjs 在專用 TEMP 下執行，結束後檢查 leftovers 才刪除，這次沒有報告遺留；finish() 會關閉瀏覽器並斷言 Chromium 與 driver 都已結束。Controller 宿主是 fake 程序，測試以 `!alive().contains(&host)` 斷言它已停止 (:91)。測試後用 tasklist 檢查：沒有 chrome-headless、ui_e2e 或 playwright 程序。仍在執行的 baton-desktop/runtime/guardian 都位於 C:\Users\User\AppData\Local\Baton\，在 12:29 已經啟動，是使用者自己安裝的 App，不是測試遺留。
- [PASS] AI 決策已追加至 ai-decisions.md — AI-46（低）、AI-47（低）、AI-48（中）以純追加方式加在檔尾，沒有改動既有條目，格式和既有條目相同（重要度、時間、決策、理由、替代方案、影響、審閱）。
TESTS:
- `pnpm build` → 結束碼 0（web build + release 建置，產出 target\release\baton-desktop.exe）
- `pnpm test` → 結束碼 0；摘要「Clippy：通過／Rust：通過／前端：通過／UI E2E：通過」；ui_e2e 4 passed，0 failed
- `BATON_PLAYWRIGHT_DIR= node scripts/test-e2e.mjs` → 結束碼 0，標為「略過（沒有執行，不代表通過）」
- `git status --short` → 執行前後都是空的，沒有改動任何追蹤中的檔案
FIXES_NEEDED: none
```

不需要修正，但有一點要讓你知道：AI-48（重要度：中）記錄了一個和 spec 的落差。spec 故事 48 寫的是「CLI 結束後等待指令自動失效」，但實機上關掉 CLI 時，背景 wait 會直接隨程序樹一起被終止，看不到結束碼 3；結束碼 3 只有在 Runtime 重啟、重新綁定後才會出現。所以清單 B5/B6 改用重啟 Runtime 來觸發。這不影響本票驗收，但如果你希望關掉 CLI 時 wait 以結束碼 3 結束，就要改宿主的 Job 設定。
exit: 0

