# 37: 知識、教訓與全文檢索

Status: completed
Blocked by: 12

**What to build:** 實作 knowledge_entries 與 lessons：依全域、Workspace、Goal、Task 範圍保存來源、證據、可信狀態、有效期間、版本與取代關係。Controller 只能提交候選教訓；使用者在「知識」分頁確認後才升為已驗證，可修正、撤銷、淘汰並保留歷史（spec D-28）。以 FTS5 + trigram 建立可重建索引，支援繁體中文子字串搜尋；FTS5 不可用時退回 LIKE 並顯示降級（spec D-26）。

**Blocked by:** 12 Artifact 儲存與成果瀏覽

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 候選教訓未經使用者確認不會被當作已驗證載入
- [x] 撤銷的教訓保留歷史且不再被搜尋為有效
- [x] 繁體中文兩字以上關鍵字可搜到決策與教訓
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
