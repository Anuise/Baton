# 53: 以 fake Port 組態補齊 UI E2E 旅程

Status: in_progress（Work-3，run R164-53-impl，2026-10-23 06:06）
Blocked by: 46, 47, 48, 49, 50

**What to build:** spec「Testing Decisions」第 3 點要求以 Playwright 做少量 UI E2E，涵蓋首次精靈、派工到完成、裁定中心批次處理、完全退出等旅程。最終 code-review（R153 第 8 項）發現其中「派工到完成」「裁定中心批次處理」兩條沒有涵蓋，原因是真實 Runtime 沒有 fake Adapter，裁定請求又只能由 Agent 憑證建立，無法在不啟動真實 Worker／Controller 的情況下走完旅程（見 AI-455）。本票設計一個只供測試使用的 Runtime 組態：以 harness 的 fake Port（scripted fake AgentAdapter、fake ProcessControl、fake QuotaObserver、fake Clock）啟動真實 Runtime 與真實 API，讓 Web UI（瀏覽器或桌面殼）透過真實 HTTP 操作，補上這兩條 E2E 旅程。這個組態不得出現在正式建置或安裝程式中。

**Blocked by:** 46～50（驗證系列已完成，避免與最終 review 修正衝突）。

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 有一個只供測試使用的 Runtime 組態，以 fake Port 啟動真實 Runtime 與 API；正式建置與 NSIS 安裝程式不包含它（以測試或建置檢查證明）
- [ ] UI E2E「派工到完成」：在 UI 建立 Workspace、確認授權、建立 Task、派工；scripted fake Worker 回報並結束；Task 經驗證後於 UI 顯示完成
- [ ] UI E2E「裁定中心批次處理」：scripted fake Worker／Controller 產生多筆裁定請求；使用者在裁定中心批次處理；每項各自保存、不形成概括授權，受影響 Task 恢復
- [ ] E2E 在沒有安裝 Playwright 時明確標為略過（不得顯示為通過），文件說明如何執行
- [ ] E2E 結束後不遺留暫存資料目錄或程序
- [ ] AI 決策已追加至 ai-decisions.md

## Comments

- 2026-10-23 主控：依最終 review 驗證 R158 的建議新增本票（AI-474）。
