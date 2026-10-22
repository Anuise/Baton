# 31: Runtime 意外崩潰後的自動恢復

Status: completed
Blocked by: 26, 15

**What to build:** 依 DR-20261021-05：Runtime 再次啟動先盤點並核對；受控停止身分已確認的舊 Controller／Worker，確認 writer 停止後以新 Run 恢復並重新檢查授權與設定；同一恢復事件最多三次嘗試（30 秒、2 分鐘退避），完成後穩定 10 分鐘才持久化成功；三次失敗後阻擋並提供「重新嘗試恢復」。嘗試序號在任何程序副作用前持久化。UI 顯示事件、次數、退避期限與受影響工作。

**Blocked by:** 26 Outbox 交付、程序核對與外部操作追蹤；15 Session 持久化、恢復與替換

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 在嘗試保存、舊程序停止、新程序啟動等邊界中斷，不突破三次上限、不重複建立 writer
- [x] 退避期間重建 Runtime 不跳過期限
- [x] 觀察期內再崩潰沿用原事件與計數
- [x] 三次失敗後重建或等待都不產生第四次；人工重試建立新輪次並保留歷史
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
