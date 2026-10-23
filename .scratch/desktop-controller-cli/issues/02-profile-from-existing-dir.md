# 02: 以既有目錄建立 Account Profile

Status: in_progress（Work-9，run R002-02-impl，2026-10-23 14:29）
Blocked by: None（可立即開始）

**What to build:** 在帳號畫面新增「以既有目錄建立 Profile」：使用者選擇一個已登入的 Claude Code 設定目錄（例如 `~/.claude-Work-3`），平台把它登記為 Account Profile 並立即核對登入身分，不必重新登入。平台記錄目錄是「平台建立」還是「外部目錄」，封存外部目錄的 Profile 不刪除目錄。User stories 64～67。

**Blocked by:** None（可立即開始）

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)　**ADR：** [ADR-0014](../../../docs/adr/0014-native-cli-controller.md)

## Acceptance criteria

- [ ] 管理 API 可用既有目錄與名稱建立 Profile；建立後立即核對登入身分，結果顯示在帳號畫面
- [ ] 目錄不存在、與其他 Profile 相同或互相包含、位於平台資料目錄的 profiles 底下時被拒，並顯示原因
- [ ] 封存外部目錄的 Profile 不刪除、不修改該目錄；平台建立的 Profile 行為不變
- [ ] 以既有目錄建立的 Profile 可正常被派工（以 harness 驗證 Worker Run 使用該目錄作為 `CLAUDE_CONFIG_DIR`）
- [ ] 以 Runtime harness 撰寫情境測試涵蓋上述行為並通過；帳號畫面的元件測試涵蓋新表單
- [ ] AI 決策已追加至 ai-decisions.md

## Comments
