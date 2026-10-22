**VERDICT: FAIL** — 票 38 的功能本身都通過，判 FAIL 只有一個原因：`rebuild/v1` 在驗證途中前進了，現在已不是 HEAD 的祖先。

驗證開始時 `rebuild/v1` 在 `0e35b07`，當時 `git merge-base --is-ancestor rebuild/v1 HEAD` 回 `ANCESTOR_OK`。驗證途中票 14 合入整合分支，`rebuild/v1` 前進到 `64940f4`（「票 14 結案」）。重新檢查得到 `NOT_ANCESTOR`；`git merge-tree` 顯示 `crates/baton-store/src/lib.rs` 有衝突，兩邊都新增了第 19 個 migration。票 14 也已經佔用 AI-196～AI-207，和本票的 AI-196～AI-201 撞號。所以本票需要重新整合，不需要改功能。

```
VERDICT: FAIL
CRITERIA:
- [FAIL] rebuild/v1 是 HEAD 的祖先 — 驗證開始時成立（rebuild/v1=0e35b07，merge-base 回 ANCESTOR_OK）；驗證途中票 14 合入，rebuild/v1 前進到 64940f4，重查回 NOT_ANCESTOR。`git merge-tree --write-tree HEAD rebuild/v1` 回報 CONFLICT (content) in crates/baton-store/src/lib.rs：票 14 新增 migration "model_overrides_and_native_session"，本票新增 "guard"，兩者都是第 19 版。ai-decisions.md 也撞號：rebuild/v1 已有 AI-196～AI-207（票 14），本票用的是 AI-196～AI-201。
- [PASS] 啟用的 Guard 在相關 Task 准入時被檢查 — crates/baton-domain/src/run.rs:711-714 在 check_dispatch 內呼叫 guard::violations，授權檢查之後、相依檢查之前。Store 的三條派工路徑都會先載入 Guard：crates/baton-store/src/scheduling.rs:96（派工要求）、:117（佇列重試）、:203（排程總覽），以及 crates/baton-store/src/runs.rs:63-65。範圍以教訓 scope 判定（crates/baton-domain/src/guard.rs 的 violations 使用 Audience::Run）；Store 只載入「啟用且教訓仍有效」的 Guard（crates/baton-store/src/guards.rs 的 load_active_guards）。正式程式中所有 RunCommand::Dispatch 都經過 scheduler（已 grep 確認）。情境測試涵蓋三種規則、範圍隔離、停用後佇列自動派出、教訓撤銷後失效、重啟後仍生效：an_enabled_guard_blocks_admission…、guards_only_check_tasks_within_their_lessons_scope、a_tool_restriction_guard…、a_retry_policy_guard…、disabling_a_guard_lets…、a_guard_stops_applying_once_its_lesson_is_revoked…、guards_and_recurrences_survive_a_runtime_restart，全部 ok。
- [PASS] Guard 擋下時 UI 顯示原因與來源教訓 — 後端：crates/baton-domain/src/scheduler.rs:193-207 為每個違反產生 category=guard、code=guard_violated 的等待原因，附 GuardSource（guard_id、lesson_id、lesson_title），訊息含「來源教訓「…」」。前端：web/src/TaskBoard.tsx:77 加了「Guard 擋下」標籤，:465-469 顯示「來源教訓：{lesson_title}」；web/src/runtimeClient.ts 的 WaitReason.guard 型別已補上。測試：TaskBoard.test.tsx 兩案例（佇列原因、派工 202 立即顯示）通過；harness 測試斷言 202 回應的 reasons[].guard.lesson_title 正確。
- [PASS] 重犯事件可追溯到教訓 — migration 新增 lesson_recurrence 表，lesson_id NOT NULL、外鍵指向 knowledge_entry，追加式，有禁止 UPDATE／DELETE 的 trigger（crates/baton-store/src/lib.rs:625-658）。scheduling.rs:162-167 在同一個交易中以 record_prevented 記錄攔下的重犯，識別為 guard+task+契約版本，同一組合只記一次。使用者可經 POST /api/v1/knowledge/{id}/recurrences 回報沒被攔下的重犯；Guard 列表顯示成效（prevented_count、last_prevented_at、reported_count）；KnowledgePage 有重犯紀錄與回報表單。測試：a_prevented_recurrence_is_recorded_once_and_traceable_to_the_lesson 與 the_user_reports_a_recurrence_that_slipped_through… 都 ok，前者另外驗證事件表的 payload 指向 lesson_id 與 guard_id。
- [PASS] Runtime harness（D-04）情境測試涵蓋本票行為並通過；AI 決策已追加 — crates/baton-harness/tests/guards.rs 共 10 個測試，全部 ok。D-28「只有使用者能建立」也有驗證：Agent 憑證得到 403 role_not_allowed，候選教訓與決策條目得到 409 lesson_ineligible。ai-decisions.md 追加了 AI-196～AI-201，都標了重要度（中／低）。但編號與整合分支衝突（見第一條）；AI-200 少了「審閱：未審」一行，和 AI-201 之間也沒有空行。
TESTS:
- `pnpm test`（等同 with-msvc cargo test --workspace --exclude baton-desktop 加 pnpm --filter web test）：exit 0。Rust 共 905 個測試通過、0 失敗，其中 tests/guards.rs 10/10、guards_client 1/1；前端 17 個測試檔、333 個測試全部通過。
- `pnpm build`：exit 0，release 建置完成，產出 target/release/baton-desktop.exe。
- 跑完建置與測試後 `git status --short` 是空的：沒有改動任何追蹤中的檔案。
- 注意：上述測試是在 HEAD ddc64bc 上跑的，那時還沒有整合票 14。
FIXES_NEEDED:
1. 把目前的 rebuild/v1（64940f4，含票 14）合併進 rebuild/38-guards。
2. 解決 crates/baton-store/src/lib.rs 的 MIGRATIONS 衝突：保留票 14 已合入的第 19 版 "model_overrides_and_native_session"，本票的 "guard" 順延為第 20 版，排在它後面。
3. 已合入整合分支的條目編號不能改，所以本票的條目重新編號：ai-decisions.md 的 AI-196～AI-201 改成 AI-208～AI-213，放在 AI-207 之後。程式碼與文件中引用這些編號的地方一併更新，包括 AI-201 內文寫的「順延為 AI-196～AI-200」，以及 AI-200 內文寫的「第 19 個 migration」（改為第 20 個）。另外新增一條整合紀錄的 AI 條目，說明這次順延。
4. 順手補上 AI-200 缺的「- 審閱：未審」一行，並和下一個條目之間空一行。
5. 確認 crates/baton-domain/src/run.rs、scheduler.rs、crates/baton-store/src/runs.rs 自動合併後的語意：票 14 改了 run.rs 的派工與設定快照准入，要確保 Guard 檢查仍在 check_dispatch 內，而且順序在授權准入之後。
6. 合併後重跑 `pnpm test` 與 `pnpm build`，兩者都要 exit 0；再確認 `git merge-base --is-ancestor rebuild/v1 HEAD` 成立。
（非阻擋，供參考：POST /api/v1/knowledge/{id}/recurrences 只帶 workspace_id、不帶 task_id 時，會靜默丟棄 workspace_id，在 crates/baton-api/src/guards.rs 的 report 函式。同一個 Guard、Task 與契約版本在停用後再啟用時，第二次攔下不會再記一筆重犯，這是 AI-199 定義的去重行為。）
```
