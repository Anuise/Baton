# 19: 多帳號並行排程與容量

Status: in_progress（Work-9，run R025-19-impl，2026-10-22 15:55）
Blocked by: 10, 07

**What to build:** Scheduler 依相依、優先序、能力、授權、帳號可用性與執行槽選擇派工：預設每帳號 2 個並行 Run、全域 6 個，帳號數 ≥3 時保留 1 個交接槽（spec D-14），設定頁可在「較高並行／保留交接容量」間切換。帳號頁顯示每個帳號的進行中 Run。

**Blocked by:** 10 Supervisor、程序身分與 Worker Run 端到端（fake Adapter）；07 帳號 Profile 與首次啟動精靈

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 兩個帳號同時執行不同 Task，Run 與帳號關聯正確
- [ ] 達到每帳號或全域上限時新 Task 排隊並顯示原因
- [ ] 交接保留槽不被一般派工佔用
- [ ] 阻擋原因區分額度、授權、相依與狀態不明
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
