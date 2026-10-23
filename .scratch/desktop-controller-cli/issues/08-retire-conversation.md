# 08: 移除舊的對話頁與 Controller 自動喚醒

Status: in_progress（Work-3，run R012-08-impl，2026-10-23 16:09）
Blocked by: 03

**What to build:** Controller 改由 Controller CLI 操作後，移除「對話」頁簽、前端對話 client 方法，以及 Runtime 的 Controller 自動喚醒（事件合併、`claude -p` 輪次、Controller 自動交接）。對話資料表保留、不遷移、不再寫入。Controller 工具與 MCP shim 不變。User stories 72～74。

**Blocked by:** 03

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)　**ADR：** [ADR-0014](../../../docs/adr/0014-native-cli-controller.md)

## Acceptance criteria

- [ ] UI 不再有「對話」頁簽與相關元件
- [ ] 使用者訊息或工作事件不再產生任何自動 Controller Run（以 harness 驗證）
- [ ] 既有對話資料仍在資料庫，升級後不遷移、不刪除
- [ ] 被移除功能的測試一併移除；其餘全部測試通過；GLOSSARY 中 conversation 相關詞條與 ADR-0009 的狀態若需調整，一併更新
- [ ] AI 決策已追加至 ai-decisions.md

## Comments
