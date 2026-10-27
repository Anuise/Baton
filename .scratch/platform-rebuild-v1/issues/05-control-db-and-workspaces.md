# 05: 控制資料庫、寫入佇列與 Workspace 建立

Status: completed
Blocked by: 04

**What to build:** 建立 SQLite 控制資料庫（spec D-17 的資料目錄、issue §11.1 PRAGMA 並於啟動時讀回驗證）、schema_migrations、Store 的單一寫入佇列與 Domain 核心「命令 → 新狀態 + 事件」的模式。第一個垂直切片：使用者在 App 中選擇目錄建立 Workspace（可以不是 Git repo），在左側清單看到它；資料位於網路共享或雲端同步資料夾時拒絕並說明。

**Blocked by:** 04 本機 API 身分驗證與網頁存取防護

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] Workspace 的建立、列出、封存經由桌面 API 與 UI 完成，資料持久化且重建 Runtime 後仍在
- [x] 所有寫入經單一寫入佇列，事件表追加對應事件；不相容的 schema 版本拒絕寫入
- [x] 啟動時驗證 WAL、foreign_keys 等設定生效
- [x] UTC 存放、以系統時區顯示（spec D-33）
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
