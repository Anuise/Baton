# 36: 修正與重試預算

Status: in_progress（Work-8，run R086-36-impl，2026-10-22 18:41）
Blocked by: 35, 13

**What to build:** 依 spec D-16：實作失敗修正 3 次、驗證不完整 2 次、基礎設施重試 5 次（指數退避上限 10 分鐘），分開計數；額度交接與恢復嘗試不計入。超過預算時 Task 進入 blocked 並建立裁定請求。Task 詳情顯示各計數。

**Blocked by:** 35 完整裁定中心與決策權限表；13 成果接受與最小完成驗證

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 三種計數互不影響；交接不增加任何計數
- [ ] 預算用盡時 Task blocked 並出現裁定請求
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
