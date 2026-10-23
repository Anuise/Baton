# 03: Controller CLI 開啟與結束（tracer bullet）

Status: ready-for-agent
Blocked by: 01

**What to build:** 在 Workspace 的 Controller 頁簽選一個 Account Profile、按「開啟」，平台就在外部終端機視窗（優先 Windows Terminal，否則系統預設主控台）啟動原生互動式 Claude Code，作為該 Workspace 的 Controller CLI；按「結束」或在終端機關掉它，頁簽狀態隨之更新。平台把它記成一個互動式 Controller Run：先持久化 launch intent 再開窗；終端機裡執行 MCP shim 的「Controller 宿主」子命令，由它啟動 `claude` 並回報開始與結束；Run 結束即撤銷 Agent 憑證。啟動內容與 Worker 的 Claude Code Adapter 共用組裝（清除繼承環境變數、`CLAUDE_CONFIG_DIR`、strict MCP 設定指向 shim 與共同設定的 MCP、設定快照、工作目錄＝Workspace），但不加 `-p`、不指定輸出格式與權限模式、憑證範圍是 Controller。操作手冊、帳號排除、事件等待、退出恢復由後續票處理。User stories 18～32、36、37、41。

**Blocked by:** 01

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)　**ADR：** [ADR-0014](../../../docs/adr/0014-native-cli-controller.md)

## Acceptance criteria

- [ ] Controller 頁簽可選可用的 Profile（不可用者不能選），按開啟後經 Claude 環境 Port 開出終端機；fake Port 記錄到的啟動規格正確（環境清理、帳號目錄、MCP 設定、工作目錄、無 `-p`、無權限模式）
- [ ] 開啟建立互動式 Controller Run（starting → running → ended），時間線記錄開啟、結束與帳號
- [ ] 同一 Workspace 已有非終態 Controller Run 時再次開啟被拒並提示；同一 Profile 可同時當兩個 Workspace 的 Controller
- [ ] 宿主回報結束、宿主消失（程序存活查驗）、使用者按結束，都讓 Run 進入 ended 並使憑證失效（之後以該憑證呼叫 Agent API 被拒）
- [ ] 在 launch intent 與開窗回執之間崩潰後重啟，不重複開窗
- [ ] 開窗失敗、找不到 `claude`、Profile 失效時，頁簽顯示原因且不留下非終態 Run
- [ ] Controller 憑證可呼叫既有的 Controller 工具（以 harness 驗證至少 `propose_task` 與 `dispatch_task`）
- [ ] 以 Runtime harness 撰寫情境測試涵蓋上述行為並通過；Controller 頁簽有元件測試；真實 Port 的開窗實作有 Windows 下可執行的煙霧測試或明確標為手動驗收
- [ ] AI 決策已追加至 ai-decisions.md

## Comments
