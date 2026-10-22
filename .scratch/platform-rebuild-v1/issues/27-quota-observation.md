# 27: 額度觀測與帳號頁

Status: ready-for-agent
Blocked by: 19

**What to build:** 依 spec D-12 實作 QuotaObserver：已觀測來源解析 CLI 的限流訊號與重置時間；推估來源以 Run 結果累計用量估算五小時與七日視窗，使用者可為 Profile 設定方案上限。每筆觀測保存來源、時間、可信度與原始資料參照，未知不填零。帳號頁顯示用量、重置時間、可信度與觀測時間；依 spec D-31 每 5 分鐘輪詢。

**Blocked by:** 19 多帳號並行排程與容量

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 觀測不到時顯示「未知」而非 0
- [ ] 已觀測與推估並存時以較保守者判斷
- [ ] 其他裝置用量不可見時明確標示觀測不完整
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
