VERDICT: PASS
CRITERIA:
- [PASS] 前置條件：`rebuild/v1` 是 HEAD 的祖先 — 驗證開始時 `git merge-base --is-ancestor rebuild/v1 HEAD` 成立（當時 `rebuild/v1` = f051660）。驗證途中整合分支前進到 263efdb，多了兩個 commit（51787a3、263efdb），兩個都只改 `.scratch` 的 run 紀錄（`chore(scratch): 記錄 R108／R104…`），沒有程式變更。依指示只標註，不判 FAIL；合併時要把這兩個 commit 一起併入。
- [PASS] Controller 透過工具建立的 Task 與使用者手動建立的 Task 受相同准入檢查 — 情境測試 `tasks_proposed_by_the_controller_pass_the_same_admission_check_as_tasks_created_by_the_user`（`crates/baton-harness/tests/controller_tools.rs:184-249`）通過。它用同一份不完整的契約各建一個 Task，比對兩者的 `admission.missing`、面板上的 `missing`，以及 `dispatch_task` 和 `/ready` 都回 409 `task_admission_denied`、缺項相同（:209-220）。程式碼方面，`propose_task` 用與桌面端點相同的 `TaskCommand::CreateTask` 和 `task::decide`（`crates/baton-store/src/tasks.rs` 的 `propose_task`）。`dispatch_task` 用 `task_admission_report`，之後交給共用的 `runs::dispatch_now`（`crates/baton-api/src/controller.rs:343-365`）；Scheduler 的派工路徑也會重新跑 `task::admission_report`（`crates/baton-domain/src/run.rs:981`）。循環相依與跨 Workspace 的情況另由 `the_controller_cannot_create_a_dependency_cycle_or_change_tasks_of_another_workspace`（:252-292）涵蓋。
- [PASS] 超出 Workspace 授權的 Task 被拒絕並要求送裁定 — `a_task_beyond_the_workspace_authorization_is_refused_and_sent_to_the_decision_center`（:295-349）通過。驗到的行為：`dispatch_task` 回 409 `requires_decision`（matter `exceed_authorization`，回應附 `decision_request_id`）；沒有建立 Run，也沒有 spawn；裁定中心出現 `kind: authority`、`origin: controller` 的請求；再呼叫一次回到同一個請求；使用者核准後仍回 `authorization_not_expanded`。實作在 `controller.rs:342-356`。
- [PASS] 依賴未回答問題的 Task 不會被派出，獨立的研究 Task 可派出（以 fake Adapter 的腳本化 Controller 驗證） — `a_task_that_depends_on_an_unanswered_question_waits_while_independent_research_is_dispatched`（:352-436）通過。驗到的行為：研究 Task 回 201 started；依賴答案的 Task 在契約補齊後仍有缺項 `decision_pending`，`dispatch_task` 和使用者的 `/ready` 都被擋；過 3 小時仍沒有被派出；只有研究 Task 被 spawn（:408-410）；使用者回答後 Controller 被喚醒，Prompt 帶有答案，接著才派出。機制是 `DecisionHold` 進入 `task::admission_report`（`crates/baton-domain/src/task.rs:485-492`），hold 由 `crates/baton-store/src/tasks.rs` 的 `load` 查詢 `origin='controller' AND kind='agent' AND status='pending'` 而來。
- [PASS] 以 Runtime harness（spec D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加至 ai-decisions.md — `crates/baton-harness/tests/controller_tools.rs` 跑真實的 Runtime、Store 和 Axum API，只把 Adapter、Clock、ProcessControl 換成 fake，6 個測試全部通過（`test result: ok. 6 passed; 0 failed`）。`ai-decisions.md` 追加了 AI-327～AI-334，每條都標了重要度（高／中／低）。

TESTS:
- `pnpm install`：完成。
- `pnpm test`（`cargo test --workspace --exclude baton-desktop` 加上 `pnpm --filter web test`）：EXIT=0。Rust 1354 個通過、0 個失敗，其中 `controller_tools.rs` 6 個全過；前端 vitest 20 個檔案、463 個測試全過；腳本輸出「Rust：通過」「前端：通過」。
- `pnpm build`：EXIT=0，產出 release 版的 `baton-runtime`、`baton-guardian`、`baton-desktop.exe`。有一個 warning：`crates/baton-api/src/shared_settings.rs:20` 有未使用的 import `StoreError`。這個檔案不在本票的 diff 內，是先前就有的問題。
- 驗證後 `git status` 是乾淨的，沒有修改任何追蹤中的檔案。

FIXES_NEEDED: none

有三點不影響判定，但值得知道：
1. Controller 憑證只能呼叫 6 個 Controller 工具和 `permission_prompt`（AI-327）。spec D-05 列為「共同」工具的 `search_knowledge`、`send_message`、`read_inbox`，Controller 都不能用；`accept_artifact` 和 Controller 版的 `record_lesson_candidate` 也還沒有實作。這些不在本票的驗收條件內，但與 spec D-05 有落差，需要由後續的票處理。
2. 票面只列 5 個工具，實作另外加了 spec D-05 裡的 `propose_decision_request`（AI-328）。「依賴未回答問題」這條驗收條件需要它。
3. Controller 的 `--allowedTools` 只有 `mcp__baton`，其他操作都會走 `permission_prompt`，而 `permission_prompt` 對 Controller 一律拒絕（AI-332）。所以「Controller 不能直接寫資料庫或建立未受管理的 Worker」成立，由 `only_the_controller_credential_can_use_controller_tools_and_it_cannot_act_as_a_worker` 驗證。
