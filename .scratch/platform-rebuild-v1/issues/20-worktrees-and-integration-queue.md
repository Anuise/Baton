# 20: Worktree、獨立輸出目錄與整合佇列

Status: ready-for-agent
Blocked by: 13

**What to build:** 開發類 Task 在平台建立並登記的獨立 git worktree 中執行；研究與文件 Task 使用獨立輸出目錄；目錄不可用時不派工並顯示原因。程式分支整合排入單一整合佇列，合併後執行整合驗證；未通過整合不能宣告完成。

**Blocked by:** 13 成果接受與最小完成驗證

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 不同開發 Task 各自擁有 worktree，與 Session／Run 關聯正確
- [ ] 整合佇列一次只處理一個合併；合併後驗證失敗時 Task 不完成
- [ ] 非 Git Workspace 的研究 Task 使用輸出目錄而非 worktree
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
