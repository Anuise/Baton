# 09: Controller CLI 的 UI E2E 與 Windows 手動驗收

Status: completed
Blocked by: 02, 04, 05, 06, 07, 08

**What to build:** 沿用 platform-rebuild-v1 票 53 的 fake Port E2E 組態，補一條旅程：選 Workspace → Controller 頁簽 → 選 Profile → 開啟 → 狀態變執行中 → 結束。另在 Windows 平台驗收清單補上需要真實環境的項目：Windows Terminal／預設主控台開窗、原生 `claude` 互動、Controller 工具可用、背景 `wait` 收到 Worker 結束通知、Controller 帳號不被派工、完全退出關窗、以既有 `.claude-Work-n` 建 Profile。

**Blocked by:** 02, 04, 05, 06, 07, 08

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)　**ADR：** [ADR-0014](../../../docs/adr/0014-native-cli-controller.md)

## Acceptance criteria

- [x] fake Port E2E 旅程通過；沒有安裝 Playwright 時明確標為略過
- [x] Windows 驗收清單新增上述手動項目，每項寫明步驟與預期結果
- [x] E2E 結束後不遺留暫存資料目錄或程序
- [x] AI 決策已追加至 ai-decisions.md

## Comments
