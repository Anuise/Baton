# 25: 桌面生命週期、系統匣與完全退出

Status: completed
Blocked by: 19

**What to build:** 依 DR-20261021-01：關閉視窗 Runtime 繼續；系統匣提供狀態、重新開啟與「完全退出」。完全退出先持久化停止意圖（同時寫入資料庫與生命週期檔，spec D-25），停止新派工、交接與重試，要求 Run 在 30 秒內保存並停止，再以 10 秒終止剩餘程序樹（spec D-11），顯示停止進度與異常。每個 Workspace 有「完全退出後自動續跑」設定，預設關閉；重新開啟先核對，再依設定續跑或等待「繼續工作」。

**Blocked by:** 19 多帳號並行排程與容量

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 關窗後 Run 持續；重開視窗連回同一 Runtime，不重複啟動 Worker
- [x] 停止意圖生效後，佇列重試、額度恢復與交接都不能啟動新 Run
- [x] 退出流程在 40 秒內結束，無法確認停止的程序被記錄為停止異常
- [x] Workspace A 開、B 關時，重開後只有 A 的合格工作自動恢復
- [x] 完全退出不會把 Task 標為取消或完成
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
