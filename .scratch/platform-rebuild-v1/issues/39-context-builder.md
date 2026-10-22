# 39: Context Builder 與 context receipt

Status: ready-for-agent
Blocked by: 37, 17

**What to build:** Context Builder 依工作相關性與 token 預算為每個 Run 組裝：Task 契約、適用決策、教訓摘要、最近檢查點、未讀訊息摘要與證據索引；重要約束優先保留。每個 Run 保存 context receipt（載入項目、版本、遺漏原因、預算），可在 Run 詳情查看。

**Blocked by:** 37 知識、教訓與全文檢索；17 Controller 拆工與派工工具、漸進拆工

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 相關的已驗證教訓在 Task 開始前被載入
- [ ] 預算不足時省略項目被記錄於 receipt
- [ ] receipt 版本與實際載入內容一致
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
