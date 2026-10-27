# 41: 事件時間線與觀測指標

Status: completed
Blocked by: 28

**What to build:** 「時間線」分頁呈現派工、交接、裁定、恢復、驗證與清理事件，可依 Task 過濾；提供指標：工作完成率、交接成功率、恢復耗時、重複操作事件、裁定等待時間、教訓重犯率、每階段用量與 Context 使用量；不可見的服務端額度顯示未知。

**Blocked by:** 28 檢查點與主動跨帳號交接

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 時間線依 Task 過濾後事件完整且有序
- [x] 每項指標可由事件重新計算並與顯示一致
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
