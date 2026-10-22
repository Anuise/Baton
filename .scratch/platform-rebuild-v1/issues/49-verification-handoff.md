# 49: 驗證中的額度交接

Status: in_progress（Work-9，run R144-49-impl，2026-10-23 02:26）
Blocked by: 46, 28

**What to build:** 驗證 Run 納入帳號額度與交接：每個可恢復步驟記錄已完成操作與判定；交接先停止舊測試程序，承接者以安全的測試 Session 參照查驗現況；無法移轉的瀏覽器 Session 從隔離 fixture 或最近確認邊界重跑；單一案例未完成不得拼接為 passed。全帳號受限時只執行不需模型的斷言或重播。

**Blocked by:** 46 Web 驗證 Adapter（Playwright）；28 檢查點與主動跨帳號交接

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 驗證中途交接後結果正確，未完成案例不會被拼成 passed
- [ ] 結果未知的外部操作不因重測而重送
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
