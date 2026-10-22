# 47: 探索測試到可重現缺陷

Status: ready-for-agent
Blocked by: 46

**What to build:** 探索 Task 先定 charter（區域、角色、目標、禁止副作用、步數與成本上限）；結果先標 suspected，保留重現步驟與證據；排除環境與預期設計後建立重現測試，以產品行為斷言失敗才升為 confirmed；無法重現者保留為 unverified risk。修復後納入正常回歸。UI 區分疑點、已確認缺陷與未驗風險。

**Blocked by:** 46 Web 驗證 Adapter（Playwright）

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 環境故障不被記為產品缺陷
- [ ] 未經重現的疑點不會顯示為 confirmed
- [ ] 不能以排除標籤或刪除案例掩蓋缺陷
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
