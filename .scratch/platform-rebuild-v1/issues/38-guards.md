# 38: Guard 與執行前檢查

Status: completed
Blocked by: 37

**What to build:** 已驗證教訓可轉為 Guard（啟動前檢查、工具限制或重試政策），由使用者啟用；Run 准入時執行適用 Guard，違反時阻擋並顯示對應教訓。記錄重犯事件與 Guard 成效。

**Blocked by:** 37 知識、教訓與全文檢索

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 啟用的 Guard 在相關 Task 准入時被檢查
- [x] Guard 擋下時 UI 顯示原因與來源教訓
- [x] 重犯事件可追溯到教訓
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
