# 15: Session 持久化、恢復與替換

Status: completed
Blocked by: 14

**What to build:** 實作 agent_sessions、session_events、transcript_segments：平台 Session 與原生 Session、程序身分分開保存；同帳號續跑以新 Run 沿用相容 Session；跨帳號或原生 Session 遺失時建立替代 Session、保存 predecessor 與原因；持續把原生對話紀錄匯入受控儲存並記錄缺口（issue §3.1～3.3）。

**Blocked by:** 14 真實 Claude Code Adapter 與 Adapter 契約測試

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 同一原生 Session 不能被兩個有效執行者同時驅動
- [x] 原生 ID 未取得前中斷，可由 command_id 核對，不建立重複 writer
- [x] 原生 Session 不可用時建立替代 Session 並標示紀錄缺口，不宣稱完整恢復
- [x] transcript 匯入以事件位置去重；憑證不進 transcript
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
