# 06: Workspace 授權範本與確認（I2／P3）

Status: in_progress（Work-8，run R013-06-impl，2026-10-22 14:48）
Blocked by: 05

**What to build:** 新 Workspace 建立後進入授權設定：提供可編輯範本（研究／文件範本、可信專案開發 P3 範本），以「能力 + 目標資源 + 限制」呈現，確認摘要揭露可信程式以目前 Windows 使用者身分執行的風險；確認並保存後授權才生效並建立 revision。未確認前，任何需要該能力的 Agent 都不能被准入（DR-20261021-03）。管理員／全域安裝、遠端寫入、部署、對外發訊、付費與正式資料操作不在 P3 中。

**Blocked by:** 05 控制資料庫、寫入佇列與 Workspace 建立

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 未確認授權的 Workspace 無法建立需要對應能力的 Run（准入檢查拒絕並顯示原因）
- [ ] 授權每次修改產生新 revision 並保存來源與時間
- [ ] P3 範本不含管理員／全域安裝、部署、對外發訊、付費、正式資料操作
- [ ] 研究／文件 Workspace 不被強制套用開發範本
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
