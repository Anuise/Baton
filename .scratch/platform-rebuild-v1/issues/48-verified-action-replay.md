# 48: 已驗證動作重播與 no_live_model

Status: ready-for-agent
Blocked by: 46

**What to build:** 只有後續有效斷言證明結果的動作才錄製；重播執行真實動作並重新檢查後置條件；快取身分含案例、目標、參數、persona、引擎與資料相容性，不一致時標記 stale。no_live_model 設定在 Adapter 層強制禁止所有 live 模型呼叫。正式環境副作用預設不可重播。重播與 live 用量分開呈現。

**Blocked by:** 46 Web 驗證 Adapter（Playwright）

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 未經斷言確認的動作不會被錄製
- [ ] stale 快取不沿用舊成功
- [ ] no_live_model 下任何模型呼叫都被阻擋（測試以 fake 模型計數驗證）
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
