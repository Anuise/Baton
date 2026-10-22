# 21: Worktree 完成後自動清理

Status: ready-for-agent
Blocked by: 20

**What to build:** 依 DR-20261021-02：Task 通過驗證、成果已整合、相關程序停止、無其他使用者、必要成果與證據已保存於 worktree 外時，自動移除平台登記的 worktree；未保存變更或歸屬不明檔案阻擋刪除。清理意圖持久化、失敗時有限重試並顯示原因；Task 完成與清理狀態分開顯示。暫停、交接、失敗、取消、完全退出不觸發清理。

**Blocked by:** 20 Worktree、獨立輸出目錄與整合佇列

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 只清理平台登記的 worktree，原始 checkout 與其他目錄不受影響
- [ ] 模擬檔案占用造成移除失敗：保留待清理狀態並重試，Task 仍顯示已完成
- [ ] 清理中崩潰後重建 Runtime，核對後完成清理且不誤刪
- [ ] 不刪除 Git 分支、歷史或正式 Artifact
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
