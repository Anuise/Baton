# 32: 監護程序、有限自動重啟與降級

Status: completed
Blocked by: 31

**What to build:** 依 DR-20261021-06 與 spec D-24：獨立輕量監護程序在同次登入期間監看 Runtime；確認意外終止後，先把事件、輪次與嘗試序號寫入 append-only 監護日誌並 fsync，再啟動 Runtime；Runtime 啟動時先匯入監護日誌並去重，與 DR-05 共用三次額度。監護不寫控制資料庫；完全退出、更新與解除安裝抑制重啟。監護死亡時 Runtime 繼續並顯示「工作繼續，自動重啟保護不可用」，下次開 App 只補監護。

**Blocked by:** 31 Runtime 意外崩潰後的自動恢復

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 關閉視窗後殺掉 Runtime，監護自行重啟並經過完整恢復核對
- [x] 監護日誌與資料庫銜接不重複扣次；啟動早期失敗也計次
- [x] 完全退出與監護啟動競態時不拉起 Runtime
- [x] 殺掉監護：工作繼續、UI 顯示降級，重開 App 只補監護
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
