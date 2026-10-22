# 45: Verification Profile 與 CLI／API 驗證 Adapter

Status: ready-for-agent
Blocked by: 43

**What to build:** 專案維護 Verification Profile（啟動命令、doctor、就緒條件、資料準備、斷言、證據保存、teardown）與 Feature Map；實作 CLI／API Verification Adapter（spec D-29）：操作與判定分離、程式斷言優先；每步驟保存 verdict 與 reason_code（passed、product_failure、inconclusive、environment_blocked、auth_blocked、budget_exhausted、automation_unsupported）；Evidence Bundle 綁定 build、revision、需求與 profile 版本。清理只移除本 Run 的狀態並保留證據。

**Blocked by:** 43 方法登錄、Playbook 路由與 Stage／Gate

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] Worker 宣稱成功但實際副作用不符時驗證失敗
- [ ] 證據不足與環境阻礙不算通過
- [ ] 新 Profile 至少實際跑過一條代表旅程，並標示未覆蓋範圍
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
