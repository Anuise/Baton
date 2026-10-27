# 26: Outbox 交付、程序核對與外部操作追蹤

Status: completed
Blocked by: 25

**What to build:** Outbox Dispatcher 至少一次交付、以 command_id 去重，每條路徑都先檢查停止意圖與有效准入；無法證明程序是否已啟動時進入 reconciliation 而非重派。實作 external_operations：冪等鍵、要求雜湊、回執與「結果未知」狀態；未知時阻擋相關 Task 並建立裁定請求，不盲目重送。SQLite busy 或磁碟滿時停止新派工並顯示原因。

**Blocked by:** 25 桌面生命週期、系統匣與完全退出

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 在程序啟動、回執、外部回應等邊界注入故障，重建後不重複啟動、不重送副作用
- [x] 外部操作結果未知時相關 Task 阻擋，獨立 Task 繼續
- [x] 磁碟滿時新派工停止，UI 顯示可處理原因
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
