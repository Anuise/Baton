# 03: Windows 平台辨識與支援狀態

Status: ready-for-agent
Blocked by: 01

**What to build:** Runtime 啟動時辨識 Windows 功能版本、build、版本別與原生架構（DR-20261021-08），判定為正式支援（Windows 11 x64 Home／Pro 25H2、26H2）或非正式支援；App 的平台狀態頁與狀態列顯示結果，非正式支援時顯示警告但不阻擋使用。支援期限資料以內建表格提供並標示來源與查閱日期，過期時採 L1 警告。

**Blocked by:** 01 專案骨架與 Runtime harness

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] PlatformInfo Port 的真實實作能辨識 ARM 上 x64 模擬，不誤判為支援平台
- [ ] harness 測試：四種支援組合顯示正式支援；Windows 10、Server、ARM 顯示非正式支援；已到期版本顯示 L1 警告且不阻擋派工
- [ ] 診斷資訊包含 OS build、版本別、原生架構、App 版本
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
