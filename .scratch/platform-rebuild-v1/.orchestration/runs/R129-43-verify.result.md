我用唯讀方式核對了第 43 張票，五條驗收條件都成立，建置與全部測試也都通過，判定 **PASS**。核對期間沒有修改任何追蹤中的檔案，`git status --porcelain` 前後都是空的。

```
VERDICT: PASS
CRITERIA:
- [PASS] 前置：rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 輸出 ANCESTOR_OK（rebuild/v1 = c777a27，HEAD 是 bf465b8 合併提交）
- [PASS] 功能、缺陷、調查分別走對應 Playbook；非開發工作不套用開發流程 — 七個內建 Playbook 在 crates/baton-domain/src/workflow/builtin.rs:282-370（ambiguous/feature/defect/migration/performance/investigation/exploratory_testing）。Router 在 crates/baton-domain/src/workflow.rs:531-571：Task 不是 development 種類卻選了開發情境時回 NotApplicable；理由裡寫入標題、「第 N 版」與 Controller 的判斷。情境測試 features_defects_and_investigations_…（crates/baton-harness/tests/methods_playbooks_stages.rs:228-296）確認：三種情境各自對到正確 Playbook 與第一個 Stage；research／document Task 選開發流程時得到 422 playbook_not_applicable；調查流程沒有任何 Stage 允許「修改程式」；同一 Task 重複開始回 409 workflow_exists；Worker 呼叫回 403 controller_only。結果 ok
- [PASS] 缺少證據時 Stage 記錄 incomplete 或 blocked — evaluate()（workflow.rs:897-923）只認經 Runtime 查驗有效的證據：裁定還沒回答 → blocked，其他缺漏 → incomplete。store 的 check_evidence（crates/baton-store/src/workflows.rs:248-286）查驗成果與裁定都限定在同一個 Workspace，Task 證據另外要求 task_verification 為 passed。測試 a_stage_without_evidence_…（:300-387）確認：只有文字 → incomplete 且不推進；引用不存在的成果 → not_found；裁定未回答 → blocked，回答後才通過；回報 blocked／failed 都要附原因。測試 skipping_a_stage_…（:389-433）確認：跳過一定要有原因，原因保存在 attempt.note；裁定與安全 Gate 不能跳過（409 stage_not_skippable，workflow.rs:1077）；失敗依 failure_route 回到指定 Stage；integration Stage 裡 Task 尚未通過成果接受時記為 incomplete（not_accepted）。資料庫另有 CHECK 確保 skipped 一定有原因
- [PASS] Playbook 升級不改變進行中的 Workflow — migration 39 的觸發器（crates/baton-store/src/lib.rs:1402-1484）禁止 playbook_version、method_version 更新或刪除；workflow_instance_pinned 禁止更改已固定的 playbook_version_id 與綁定，stage_attempt 只能追加。測試 a_playbook_upgrade_…（:437-498）確認：發布第 2 版後第 1 版內容與雜湊不變；進行中的 Workflow 仍是第 1 版，照第 1 版的 Stage 順序推進；新開始的 Workflow 才用第 2 版；重啟 Runtime 後不多出新版本，進行中的仍是第 1 版
- [PASS] 換帳號不會讓 Workflow 從頭跑 — Controller 背景與 Worker Prompt 都帶入目前 Stage（conversation.rs 的 build_prompt、context.rs 的 ItemKind::Stage）。測試 switching_accounts_…（:502-556）確認：帳號 A 用到 0.9 後交接到 B；B 的 Prompt 從 evidence_uncertainty 接續並列出已完成的 Stage；對已通過的 Stage 回報得到 409 stage_not_current；每筆 attempt 記下各自的帳號；Worker Prompt 與 context receipt 帶的是目前 Stage；重啟後仍停在同一個 Stage
- [PASS] 以 Runtime harness（spec D-04）寫的情境測試涵蓋本票並通過；AI 決策已追加 — 上述 5 個情境測試全部 ok（5 passed; 0 failed），走的是 harness 的真實 HTTP API。ai-decisions.md 新增 AI-381～AI-390，每條都標了重要度（AI-385 為高），編號沒有重複

TESTS:
- `pnpm test`（scripts/test.mjs：`cargo test --workspace --exclude baton-desktop` + `pnpm --filter web test`）→ 結束碼 0。Rust 所有 test result 合計 1509 passed、0 failed；前端 vitest 27 個檔案、575 個測試全數通過，含 WorkflowPage.test.tsx 與 runtimeClient 的流程路徑
- `pnpm build` → 結束碼 0，產出 target/release/baton-runtime.exe、baton-guardian.exe、baton-desktop.exe（Web UI 先經 tsc 型別檢查）。唯一的警告是 crates/baton-api/src/shared_settings.rs:20 有未使用的 `StoreError`，rebuild/v1 本來就有，不是本票造成的
- 驗證前後 `git status --porcelain` 都是空的

FIXES_NEEDED: none
```

非阻擋觀察，不影響判定：

1. 某個 Stage 的失敗次數用盡預算而被阻擋後，Controller 只要附上有效證據回報 passed，仍可讓它通過並解除阻擋。spec 把「預算用盡後的下一步」列為必須送裁定的事項，兩者有出入，但不在本票的驗收條件內。
2. 成果證據只要求是同一個 Workspace 已登記的成果，不檢查內容和這個 exit check 是否相關。AI-384 已記錄這套判定規則。
