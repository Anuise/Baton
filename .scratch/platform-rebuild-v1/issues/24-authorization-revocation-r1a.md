# 24: 授權撤銷與啟動准入順序（R1-a）

Status: ready-for-agent
Blocked by: 23, 19

**What to build:** 授權更新與 Run 啟動准入經寫入佇列建立可查驗順序：撤銷前已持久化准入的 Run 沿用原授權與設定版本，未准入者（即使已排隊或有 Outbox）用新版重新檢查；新交接與恢復 Run 也用新版。UI 顯示「後續執行已停用，仍有 N 個 Run 沿用舊授權」與相關 Run 的暫停／取消入口。

**Blocked by:** 23 單次例外批准（E1）；19 多帳號並行排程與容量

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 授權更新與准入競態測試：未准入 Run 無法取得舊授權
- [ ] 已准入 Run 沿用舊版本並在 UI 標示
- [ ] 舊 Run／Session 不能成為新工作的權限捷徑
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments

- 2026-10-22 主控：票 06 的 `check_admission` 走唯讀連線（AI-53）；本票建立撤銷與准入的持久化順序時，要把它移進寫入佇列的交易。
