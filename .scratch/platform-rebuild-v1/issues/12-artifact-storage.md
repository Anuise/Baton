# 12: Artifact 儲存與成果瀏覽

Status: ready-for-agent
Blocked by: 11

**What to build:** Worker 透過 register_artifact 工具登記成果：檔案先寫暫存、fsync、原子改名，再提交含位置、大小、雜湊、版本、產生者與 Task 關聯的 manifest；未登記的暫存檔於恢復時可辨識。App 的「成果與驗證」分頁可瀏覽、預覽與下載成果（下載同樣驗證身分）。

**Blocked by:** 11 Agent API、範圍憑證與 MCP 工具 shim

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 成果寫入中途中斷後重建 Runtime，不出現已登記但內容不完整的成果
- [ ] manifest 雜湊與檔案一致；同一成果新版本不覆寫舊版本
- [ ] 未授權呼叫者無法下載成果
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
