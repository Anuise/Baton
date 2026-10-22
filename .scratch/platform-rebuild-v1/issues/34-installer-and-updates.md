# 34: 安裝程式、更新與解除安裝

Status: ready-for-agent
Blocked by: 33

**What to build:** 依 spec D-19：Tauri NSIS 單使用者安裝（免系統管理員）；啟動時與每日檢查簽章更新，只通知；使用者同意後寫入維護抑制、執行完全退出流程、備份、套用、依 DR-01 重新開啟。解除安裝前執行停止流程、移除登入啟動機碼，資料目錄預設保留。

**Blocked by:** 33 跨 Windows 登入的啟動、續跑與停止抑制

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 一般使用者帳號可完成安裝與啟動
- [ ] 更新期間監護與登入啟動不會拉起 Runtime
- [ ] 解除安裝後不留下登入啟動機碼；資料目錄保留
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
