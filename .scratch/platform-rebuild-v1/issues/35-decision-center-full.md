# 35: 完整裁定中心與決策權限表

Status: in_progress（Work-3，run R064-35-impl，2026-10-22 17:42）
Blocked by: 22

**What to build:** 裁定中心先顯示總覽再逐項處理：接受、修改條件、回答、要求調查、延後、駁回；批次處理逐項保存對應；相似問題合併或連結；提交時以 expected_revision 校驗，過期請求標記 superseded。內建 spec D-15 的決策權限表，Workspace 可收窄不可放寬，Controller 嘗試自行處理「必須送裁定」事項時被拒絕。狀態列顯示待裁定數（spec D-21）。

**Blocked by:** 22 最小裁定中心與權限提示轉接

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 隔天開啟裁定中心，請求完整；目標版本已變更的請求不能套用
- [ ] 批次裁定每項各自保存，不形成概括授權
- [ ] Controller 試圖修改完成條件或跳過 Gate 被拒絕並轉為裁定請求
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
