# 28: 檢查點與主動跨帳號交接

Status: in_progress（Work-8，run R094-28-impl，2026-10-22 19:17）
Blocked by: 27, 15

**What to build:** 依 issue §5.2 五步驟與 spec D-13：推估用量 ≥75% 停止對該帳號派新工作、≥85% 或觀測到限流警告即進入 draining，選擇具相同能力與權限的承接帳號；在安全邊界保存檢查點，確認舊程序停止後以條件更新轉移世代、建立 successor Run 與 handoff 紀錄；新 Worker 確認接手。交接不消耗修正預算；同 Task 換帳號冷卻 10 分鐘；<60% 或重置後恢復可派。

**Blocked by:** 27 額度觀測與帳號頁；15 Session 持久化、恢復與替換

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 模擬接近五小時與週上限，皆觸發 draining 與交接，成果延續
- [ ] 舊 writer 未確認停止前，新 Worker 不能進入同一可寫目錄
- [ ] 交接中各 phase 中斷後重建，依 phase 恢復且不出現兩個 writer
- [ ] 交接不增加修正計數
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments

- 2026-10-22 主控：票 27 驗證 R046 的觀察——`crates/baton-domain/src/quota.rs` 判斷視窗是否缺少已觀測用量時用 `judged_used().is_none()`，沒有數字的 warning 訊號會回 `Some(0.75)` 而被當成完整，與 AI-124 不一致。請在本票改成「`used.is_some()` 或 `status == Exhausted`」並補 Domain 測試。
