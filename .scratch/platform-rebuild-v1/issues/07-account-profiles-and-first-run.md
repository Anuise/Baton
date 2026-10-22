# 07: 帳號 Profile 與首次啟動精靈

Status: in_progress（Work-3，run R014-07-impl，2026-10-22 14:48）
Blocked by: 05, 03

**What to build:** 使用者可新增最多 8 個 account_profile（spec D-32），每個綁定獨立的 CLAUDE_CONFIG_DIR；首次啟動精靈依 spec D-20 檢查 Claude Code 是否在 PATH 與版本、Git for Windows、各 Profile 是否已登入，缺項只顯示官方指令與「重新檢查」，不代為安裝；精靈可在 App 開啟的終端機中以該 Profile 執行原生登入。新增後核對實際認證來源與帳號身分（環境變數或 credential helper 可能覆蓋），結果顯示於帳號頁。首次使用告知背景執行行為。

**Blocked by:** 05 控制資料庫、寫入佇列與 Workspace 建立；03 Windows 平台辨識與支援狀態

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 每個 Profile 的原生設定目錄互不共用；登入資料不進資料庫、日誌或匯出
- [ ] 實際認證身分與 Profile 預期不符時標示並阻擋該 Profile 被派工
- [ ] 精靈缺項時顯示可行動指引並可重新檢查；不執行任何下載腳本
- [ ] 第 9 個 Profile 被拒絕並說明上限
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
