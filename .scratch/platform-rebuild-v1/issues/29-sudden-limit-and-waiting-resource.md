# 29: 突然限流與全部帳號受限

Status: completed
Blocked by: 28

**What to build:** 帳號突然限流時，保存事件、查驗程序與成果，由其他帳號從最近有效檢查點接手，不依賴最後一輪摘要；所有合適帳號都受限時 Task 進入 waiting_resource，有重置時間時於重置後 1 分鐘重評，未知時以 5 分鐘起、上限 60 分鐘退避。完全退出的停止意圖優先於資源喚醒。

**Blocked by:** 28 檢查點與主動跨帳號交接

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 突然限流後由另一帳號從檢查點接手
- [x] 全部受限時工作保存並等待，UI 顯示預計重評時間
- [x] 資源恢復不會解除完全退出
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
