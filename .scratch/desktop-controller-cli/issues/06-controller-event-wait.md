# 06: Controller 事件等待（背景 wait）

Status: in_progress（Work-8，run R010-06-impl，2026-10-23 16:09）
Blocked by: 03

**What to build:** Controller 派工後在 Claude Code 的背景 Bash 執行 MCP shim 的 `wait` 子命令；Worker Run 結束、Task 受阻、Decision Request 被裁定、Controller 收到訊息時，`wait` 印出事件摘要並結束，由 Claude Code 原生通知 Controller。Agent API 新增 Controller 專用的長輪詢等待端點，依 Controller Run 保存事件游標，同一事件只回報一次。憑證只從環境變數取得。User stories 42～48。

**Blocked by:** 03

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)　**ADR：** [ADR-0014](../../../docs/adr/0014-native-cli-controller.md)

## Acceptance criteria

- [ ] 等待端點：啟動時已有未確認事件就立即回傳；否則等到事件或逾時（以 fake Clock 驗證，無真實 sleep）
- [ ] 確認後同一事件不再回傳；四種事件（Run 結束、Task 受阻、裁定被回答、收到訊息）都會觸發
- [ ] Worker 憑證不能呼叫等待端點；Controller Run 結束後憑證被拒
- [ ] `wait` 子命令：有事件以 0 結束並印出摘要；逾時與憑證失效各有固定且不同的結束碼；憑證不出現在參數或輸出
- [ ] 以 Runtime harness 撰寫情境測試涵蓋端點並通過；shim 子命令沿用 shim 既有測試方式涵蓋結束碼與輸出
- [ ] AI 決策已追加至 ai-decisions.md

## Comments
