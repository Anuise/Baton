# 13: 成果接受與最小完成驗證

Status: ready-for-agent
Blocked by: 12

**What to build:** Task 只能在「完成與驗證條件」有對應證據時進入 completed：Worker 回報結束後 Task 進入 verifying，以契約中宣告的確定性檢查（命令結束碼、檔案存在與雜湊等）或使用者手動接受完成驗證；成果接受交易同時校驗 Run generation、成果 manifest 與契約版本。Worker 宣稱成功或程序正常退出都不足以完成。

**Blocked by:** 12 Artifact 儲存與成果瀏覽

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 只有文字成功或程序正常退出時 Task 不會變成 completed
- [ ] 過期 Run 的成果回報不能推進 Task
- [ ] 確定性檢查失敗時 Task 回到可修正狀態並保留證據
- [ ] 使用者手動接受需經桌面憑證，並記錄於事件
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
