# 23: 單次例外批准（E1）

Status: ready-for-agent
Blocked by: 22

**What to build:** 裁定介面區分「允許本次操作」與「修改 Workspace 授權」；E1 批准綁定單一邏輯操作識別、目標資源、重要參數、要求雜湊與成果版本，執行時再核對；改變任何一項不能沿用；操作完成後不可重用。修改 Workspace 授權須顯示新增能力與影響並明確確認。

**Blocked by:** 22 最小裁定中心與權限提示轉接

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 相同操作在換帳號或 Run 後仍可沿用批准，但參數或成果版本改變時拒絕
- [ ] 同一批准不能用於第二次操作；結果未知時先查證
- [ ] 單次批准不增加 Workspace 授權
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
