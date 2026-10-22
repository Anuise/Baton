# ADR-0013：以本機 MCP 工具伺服器作為 Agent 與 Runtime 的工具介面

- 來源識別：D-05
- 狀態：**AI 暫定，待使用者確認**（spec 的 Claude 決策 D-05；改動成本：高）
- 來源：[spec.md](../../.scratch/platform-rebuild-v1/spec.md)「Claude 決策索引」D-05、「Claude Code Runtime Adapter」；相關要求見 [issue #3](https://github.com/Anuise/issue-orchestrator/issues/3) §2、§2.3、§2.4
- 相關：[ADR-0003](0003-workspace-authorization-and-versioned-config.md)、[ADR-0004](0004-local-desktop-entry-and-api-access.md)、[ADR-0009](0009-controller-role-and-event-driven-turns.md)

## 背景

issue #3 規定 Controller 與 Worker「透過受範圍限制的工具 API 查詢和回報」，不直接修改控制資料，且 Agent 憑證只能提出與查詢、不能冒充人工批准。但 issue 沒有指定這個工具 API 在 Claude Code 中以什麼形式提供。Agent 需要一個它原生就能呼叫的介面，同時 Runtime 必須能辨識每次呼叫是哪個 Run 發出的。

## 決策

Runtime 提供本機 MCP 工具伺服器，並為每個 Run 注入專屬設定：

- Runtime 為每個 Run 產生專屬的 MCP 設定，指向一個獨立的輕量 MCP shim；shim 由 Claude Code 以 stdio 啟動，從環境變數取得該 Run 的範圍憑證，再透過 loopback 呼叫 Runtime 的 Agent API。Agent 憑證只走環境變數，Run 結束或被接替後失效。
- 第一版工具集：
  - **共同**：`get_task_contract`、`report_progress`、`save_checkpoint`、`register_artifact`、`report_blocked`、`request_decision`、`send_message`、`read_inbox`、`search_knowledge`。
  - **Controller 專屬**：`propose_task`、`update_task`、`add_dependency`、`dispatch_task`、`cancel_task`、`propose_decision_request`、`record_lesson_candidate`、`accept_artifact`（仍受驗證 Gate 約束）。
  - **權限轉接**：`permission_prompt`（Worker 的權限提示轉給 Runtime 依授權判定，範圍內自動允許並記錄，範圍外建立裁定請求）。
- 桌面管理 API 與 Agent API 分開路由、分開憑證；Agent 憑證不能批准裁定、擴大授權或取得管理能力。
- Worker 間訊息由 `read_inbox` 拉取，Runtime 不在 Worker 執行中途插話。

## 替代方案

- Agent 直接呼叫 HTTP API（透過 shell 或 curl）：憑證會暴露在命令列與 transcript 中，且需要額外教 Agent 呼叫方式。
- 解析 Agent 的文字輸出來取得動作：脆弱，無法驗證呼叫身分。
- 讓 Agent 直接讀寫控制資料：違反單一寫入者原則與範圍限制。
- 為每個 Run 常駐一個 MCP 伺服器程序：增加程序管理與恢復負擔，shim 加 Agent API 已足夠。

## 後果

- 多一個需要打包、版本相容與測試的元件（MCP shim）；它必須通過 Adapter 契約測試。
- 依賴 Claude Code 的 MCP 設定與 `permission-prompt` 工具行為；具體 CLI 參數與最低版本為 Validation Required，Adapter 啟動前以版本探測決定能力，不支援時阻擋並顯示原因，不改為全面放行。
- 工具集是平台對 Agent 的正式契約，新增或改名工具會影響 Prompt、契約測試與已保存的 Playbook，改動成本高。
- 本決策為 AI 暫定。`ai-decisions.md` 的「Spec 決策審閱紀錄」載有使用者於 2026-10-22 對 D-05 的認同，但本 ADR 在使用者於 ADR 層級確認前，仍依票 02 的驗收條件標示為待確認（見 AI-22）。
