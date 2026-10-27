# 43: 方法登錄、Playbook 路由與 Stage／Gate

Status: completed
Blocked by: 42, 13

**What to build:** 實作 Method Registry、playbook_versions、workflow_instances 與 stage_attempts：內建 issue §9.1 的七個 Playbook；Router 依情境選擇並記錄理由與版本；Stage 宣告 entry_conditions、expected_outputs、allowed_actions、budget、exit_checks、failure_route；Runtime 依證據推進 Stage，跳過須保存原因且不能跳過使用者明定的裁定或安全門檻。版本不可原地覆寫，進行中的 Workflow 維持原版本。App 新增流程視圖。

**Blocked by:** 42 Project Brief、Glossary 與 Project Map；13 成果接受與最小完成驗證

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 功能、缺陷、調查分別走對應 Playbook；非開發工作不套用開發流程
- [x] 缺少證據時 Stage 記錄 incomplete 或 blocked
- [x] Playbook 升級不改變進行中的 Workflow
- [x] 換帳號不會讓 Workflow 從頭跑
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
