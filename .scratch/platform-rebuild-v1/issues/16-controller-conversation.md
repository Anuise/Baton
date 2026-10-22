# 16: Controller 長期對話與事件驅動

Status: ready-for-agent
Blocked by: 15, 11

**What to build:** 在 Workspace 的「對話」分頁與 Controller 對話：conversation 與 conversation_turns 持久化；需要 Controller 判斷時（使用者訊息、Worker 成果或阻礙），合併 5 秒視窗後啟動一輪 `claude -p` 並 resume 原生 Session（spec D-06）。同一對話同時最多一輪（B1）；忙碌時新訊息顯示「已收到／待處理」，本輪結束後先處理使用者訊息再處理工作事件；沒有事件時不喚醒。暫停／取消不排隊（DR-20261022-01）。

**Blocked by:** 15 Session 持久化、恢復與替換；11 Agent API、範圍憑證與 MCP 工具 shim

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] Controller 忙碌時收到兩則使用者訊息與一個工作事件，處理順序正確且不並行
- [ ] 無事件時不產生任何 Controller Run
- [ ] Controller 換 Session 後 conversation_id 與未處理指示保留且不重複執行
- [ ] 在喚起與結果保存邊界中斷後，未完成事件可找回
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments

- 2026-10-22 主控：票 37 的 AI-140（原 AI-130，整合時順延）——目前沒有 Controller 憑證，`record_lesson_candidate` 暫時開放給所有 Agent 憑證（Worker 也能提交候選教訓）。本票建立 Controller 角色時，請決定是否收窄到 Controller。
