# 33: 跨 Windows 登入的啟動、續跑與停止抑制

Status: in_progress（Work-8，run R110-33-impl，2026-10-22 20:44）
Blocked by: 32, 25

**What to build:** 依 DR-20261021-07 與 spec D-25：「登入時自動啟動」（預設關閉）以 HKCU Run 機碼啟動 launcher；launcher 先讀生命週期檔，有完全退出或維護抑制時直接結束。每個 Workspace 另有「跨登入自動續跑」設定（預設關閉），與完全退出後續跑設定分開。完全退出後只有手動開 App 才依 DR-01 處理。重新登入不重設恢復額度與阻擋。

**Blocked by:** 32 監護程序、有限自動重啟與降級；25 桌面生命週期、系統匣與完全退出

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 兩項新設定預設關閉，與 DR-01 設定分開保存與顯示
- [ ] 完全退出後重新登入不啟動 Runtime、監護或工作
- [ ] Workspace 跨登入設定相反的情境測試不會互相誤用
- [ ] 登入自啟動與手動開 App 同時發生時只有一個控制平面
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
