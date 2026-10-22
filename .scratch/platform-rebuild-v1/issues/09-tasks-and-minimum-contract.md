# 09: Task、六項最小派工契約與工作面板

Status: completed
Blocked by: 06

**What to build:** 建立 Goal 與 Task 模型、Task 狀態機（pending、ready、running、waiting_decision、waiting_resource、blocked、verifying、completed、cancelled）、task_dependencies（禁止自我依賴、檢查循環）與 DR-20261022-03 的六項最小契約。使用者可在工作面板手動建立 Task（此時 Controller 尚未存在），面板依狀態分欄顯示；契約不完整、引用失效或授權不符時，准入檢查拒絕並列出缺項。

**Blocked by:** 06 Workspace 授權範本與確認（I2／P3）

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 六項契約欄位可保存引用與版本；「不適用」須附理由，「未知」明確標示
- [x] 新增造成循環的相依被拒絕
- [x] 契約不完整的 Task 不能進入 ready，面板顯示缺少項目
- [x] 契約變更保存版本歷史
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
