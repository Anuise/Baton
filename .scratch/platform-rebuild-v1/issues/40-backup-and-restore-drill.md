# 40: 備份與還原演練

Status: ready-for-agent
Blocked by: 12

**What to build:** 依 spec D-18：以 SQLite Online Backup API 建立一致快照，連同被引用成果的 manifest；觸發時機為每日、migration 前、重要裁定或 Stage 完成後（每小時最多一次）；保留 7 份每日與 4 份每週；每週自動在隔離目錄還原並核對資料庫完整性、成果雜湊與關聯，結果顯示於設定頁。備份期間暫停成果清理；備份不含登入秘密。還原後舊 Run 不自動重做外部操作。

**Blocked by:** 12 Artifact 儲存與成果瀏覽

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 還原演練在乾淨目錄成功，資料與成果雜湊一致
- [ ] 備份內容不含任何憑證或原生登入資料（掃描測試）
- [ ] 還原後外部操作不被自動重放
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
