# 10: Supervisor、程序身分與 Worker Run 端到端（fake Adapter）

Status: ready-for-agent
Blocked by: 09

**What to build:** 打通「派工 → Run → 程序 → 結束」：派工交易建立 Run、execution lease 與派工 Outbox；Supervisor 在交易外持久化 launch intent 與 command_id 後啟動程序，記錄程序身分（PID + 建立時間 + 指令雜湊，spec D-10）與回執；真實 ProcessControl 以每個 Run 一個 Windows Job Object 管理程序樹。以 fake AgentAdapter 驗證 Run 狀態（queued、starting、running、succeeded、failed、interrupted、cancelled）與 heartbeat／lease（spec D-31）。使用者可在面板派出 Task、取消 Run。

**Blocked by:** 09 Task、六項最小派工契約與工作面板

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 同一 Task 任何時刻最多一個有效寫入所有權（性質測試）
- [ ] 啟動後回執未寫回即重建 Runtime：以 command_id 找回原執行，不重複啟動
- [ ] 取消會終止整個程序樹（真實 ProcessControl 的整合測試於 Windows 執行）
- [ ] 程序退出只產生 Run 結束事件，不把 Task 標為 completed
- [ ] lease 過期只觸發核對，不直接判定程序死亡
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments

- 2026-10-22 主控：票 06 已提供 Domain `authorization::admit` 與 `Store::check_admission`（AI-53）。本票的派工交易必須呼叫同一個 `admit`，不要另寫准入邏輯。
