```
VERDICT: PASS
CRITERIA:
- [PASS] 前置條件：rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 成功（rebuild/v1=82141ce，HEAD=98fcf37 是合併提交）。驗證結束時再查一次仍成立。
- [PASS] build 改變後，受影響的 Gate 失效，獨立的 Gate 不受影響 — 邏輯在 crates/baton-domain/src/workflow/gate.rs:64-93，用通過時記下的 build 和目前的 build 比較，只作用在需要審查或驗證證據的 Stage；revalidate（gate.rs:96-156）會退回最早失效的 Stage，並記下 resume_stage。Store 端在 crates/baton-store/src/workflows.rs 的 revalidate_one，失效時同時排入重驗的驗證要求（queue_reverification），並以工作事件通知 Controller。Supervisor 每一輪都會跑一次（crates/baton-runtime/src/supervisor.rs revalidate_gates，排在 run_requested_verifications 之前）。情境測試 crates/baton-harness/tests/gate_invalidation.rs:318-378 確認了下列行為：A 的 build 從 a1 變成 a2 後，A 退回 real_interface，approval 仍是 passed；B 的 revision 不變；系統自動排入重驗，新 Bundle 的 candidate_revision 是 a2；build 沒再變時不會重複失效。domain 單元測試 gate_tests.rs:119、154 也涵蓋這部分。
- [PASS] 主控轉交的「Bundle 沒有比對 revision／build」已處理 — crates/baton-store/src/verification_profiles.rs:162-169 中，Bundle 的 candidate_revision 若不等於受驗目錄目前的 revision，回傳 EvidenceStatus::BuildChanged。API 在回報前先觀察 build（crates/baton-api/src/workflows.rs:291-299）。情境測試 gate_invalidation.rs:366-369 確認：用舊 Bundle 回報時，結果是 incomplete，證據狀態是 build_changed。gate_tests.rs:181 也有涵蓋。
- [PASS] 決策變更後，過期的裁定請求被撤銷並重新整理 — 回答時的處理在 crates/baton-domain/src/decision.rs 的 answer：答案推翻建議時，依據該假設的 Task 的待處理請求在同一個交易中標為 superseded，並通知 Controller。重新整理時，staleness 會用 overturned_basis_after 判定。情境測試 gate_invalidation.rs:383-448 確認：about_api 變成 superseded，獨立的 about_report 仍是 pending；受影響的 Task 派工時回 reference_invalid，獨立的 Task 只回 decision_pending；驗證 Gate 失效，更新契約前回報會是 blocked，更新契約後可以通過。另有單元測試 decision/tests.rs（answer、accept 建議、refresh 三項）與 task.rs a_task_built_on_an_overturned_assumption_cannot_be_admitted。
- [PASS] Agent 無法改寫完成條件讓舊成果通過 — 成果接受的證據現在必須對應目前的契約版本（workflows.rs 的 check_evidence TaskAccepted 用 JOIN 比對 contract_version）。情境測試 gate_invalidation.rs:453-492 確認：Controller 想放寬完成條件時，被拒為 requires_decision，Gate 不變；Agent 呼叫 publish_verification_profile 被拒；使用者核准、契約改版後，Gate 失效，用舊 Bundle 回報得到 not_passed，用新需求版本（requirement_version=2）下的重驗 Bundle 才能通過。Verification Profile 改變的情況由 gate_invalidation.rs:497-520 涵蓋。
- [PASS] 以主要 seam（Runtime harness，D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加 — crates/baton-harness/tests/gate_invalidation.rs 的 4 個測試全部通過，都走真實 HTTP API 和 fake Port。ai-decisions.md 新增 AI-408～AI-415，每條都標了重要度（高、中、低）。
TESTS:
- `node scripts/test.mjs`（即 pnpm test）→ 結束碼 0。Rust 共 1569 個通過、0 失敗（gate_invalidation 4/4 通過）；前端 vitest 29 個檔案、608 個測試全過。
- `node scripts/build.mjs`（即 pnpm build）→ 結束碼 0。release 版的 baton-runtime、baton-guardian、baton-desktop.exe 都產生了，web 端的 tsc --noEmit 加 vite build 也成功。
- 驗證後 `git status` 是乾淨的，沒有修改任何追蹤中的檔案。
FIXES_NEEDED: none
```

以下是不影響判定的小問題，供之後順手修：

1. crates/baton-harness/tests/gate_invalidation.rs:54 的 `error_code` 沒有被使用，編譯時會出 dead_code 警告。這是本票新增的程式碼。
2. 同一個檔案 413-414 行取得了 `/decisions` 清單，但沒有對它做任何斷言。「重新整理時判定過期」目前只有 domain 單元測試在驗。
3. 已完成的 Workflow 不會重新查驗，所以完成後才改的程式碼不會讓它重新開啟。這是 AI-408 明確記下的取捨，不算違反 spec。
4. crates/baton-api/src/shared_settings.rs:20 有未使用的 `StoreError` 匯入警告。這個檔案不在本票 diff 內，是之前就有的。
