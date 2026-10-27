# 14: 真實 Claude Code Adapter 與 Adapter 契約測試

Status: completed
Blocked by: 10, 08

**What to build:** 實作 Windows 原生 Claude Code 的 AgentAdapter：以 `claude -p` 搭配 stream-json 啟動 Worker、注入 Run 專屬設定快照（D-09）、MCP 設定（D-05）、Profile 的 CLAUDE_CONFIG_DIR、清除會改變認證來源的繼承環境變數；解析原生 Session 識別、結果、錯誤與限流訊號；以版本探測決定能力，不支援時阻擋並說明。建立一套同時跑在 fake 與真實 Adapter 上的契約測試；真實版只在已登入的 Windows 環境以手動或夜間方式執行，結果記錄 CLI 版本。模型依 Workspace 角色覆寫設定（spec D-30）。

**Blocked by:** 10 Supervisor、程序身分與 Worker Run 端到端（fake Adapter）；08 共同設定匯入與版本化快照（S3）

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 契約測試在 fake Adapter 上全部通過；真實 Adapter 的執行方式與結果記錄文件化
- [x] 首次取得 native_session_id 後以事件條件綁定，不靜默改指其他原生 Session
- [x] CLI 版本不符時阻擋該 Profile 的派工並顯示原因
- [x] 啟動參數中不包含任何憑證值
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments

- 2026-10-22 主控：票 08 驗證 R040 的觀察——`materialize_run_settings` 同一 Run 並行要求不同版本時共用 staging 目錄；已綁定的 Run 再要求快照時不核對目錄內容。本票接上准入流程時，請依 Run 序列化並在啟動前依 manifest.json 核對快照。

- 2026-10-22 主控：票 22 驗證 R052 的觀察——P3 授權下，寫入 Workspace 內的 `.claude/settings*.json` 會被自動允許（`.git` 有擋）。若 Adapter 啟動時載入專案層級設定，Worker 可藉此改變自己的權限；請在本票處理。
