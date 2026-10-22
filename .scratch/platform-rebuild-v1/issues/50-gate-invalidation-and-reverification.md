# 50: Gate 失效與重驗

Status: ready-for-agent
Blocked by: 44, 45

**What to build:** 規格、build、Verification Profile 或政策改變時，依依賴使受影響 Gate 失效並建立重驗工作；使用者答案改變既有假設時，受影響 Task 與成果被阻擋或重驗，獨立工作繼續；不得改寫完成條件讓舊成果通過。

**Blocked by:** 44 雙軸 Review、獨立 reviewer 與設計偏離；45 Verification Profile 與 CLI／API 驗證 Adapter

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] build 改變後受影響 Gate 失效、獨立 Gate 不受影響
- [ ] 決策變更後過期的裁定請求被撤銷並重新整理
- [ ] 完成條件不能被 Agent 改寫以讓舊成果通過
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments

- 2026-10-23 主控：票 45 驗證 R132 的觀察——Verification Bundle 沒有比對目前的 revision／build，驗證通過後再改程式碼，舊 Bundle 仍有效；本票的 Gate 失效與重驗請一併涵蓋。
