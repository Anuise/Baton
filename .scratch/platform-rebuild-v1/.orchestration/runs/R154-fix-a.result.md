RESULT: DONE
COMMITS: 6d62b24..66549e3（共 40 個，含 3 個子代理分支與 `rebuild/v1` 的 merge commit；`rebuild/v1` 已併入，合併後沒有新的 commit）
TESTS:
- `pnpm test`：通過。Rust 有 113 個測試目標，共 1704 個測試全部通過；前端 30 個測試檔、636 個測試全部通過。
- `pnpm --filter web typecheck`：通過。
- `cargo check -p baton-desktop`：通過。
- 工作樹乾淨，沒有 push，`main` 與 `rebuild/v1` 都沒動。

FIXED:
- R151 第 1 項【高】 — 用 migration 46 移除 `task_verification_one_pass` 唯一索引，改為每個 Run 最多一筆通過（AI-449）— store `each_run_may_pass_once_and_a_redone_run_of_the_same_task_may_pass_again`、harness `worktrees::a_task_redone_after_a_failed_integration_can_pass_verification_again_and_complete`
- R151 第 2 項【高】 — 監護日誌的 event_id 已被用過時，改以 attempt_id 建立新事件（AI-452）— store `a_guardian_record_pointing_at_a_closed_event_…`、`…_older_closed_event_…`；domain `importing_an_attempt_whose_event_id_is_taken_…`
- R151 第 3 項【高】 — 重現案例只能用 Profile 中完全相同的命令與含秘密的操作，網址的來源也必須在 Profile 中出現過（AI-450）— `exploration::tests::a_reproduction_case_stays_within_what_the_user_published_in_the_profile`
- R151 第 4、5 項 — 已要求取消的 Run，失敗、程序已死、突然限流這幾條路徑都改為完成取消；交接遇到取消就放棄（AI-453）— `run::tests::a_launch_failure_after_a_cancel_request_…` 等 3 個、`handoffs::tests::a_cancel_request_…` 2 個
- R151 第 6 項 — 建立交接與進入 stopping 時，名額計算扣掉即將結束的舊 Run — `handoffs::tests::a_handoff_is_requested_even_when_the_global_capacity_is_full_…` 等 2 個
- R151 第 7 項 — 只有已觀測來源的 reset_at 才算重置（AI-454）— `handoff::tests::an_estimated_reset_time_is_not_an_observed_reset_and_keeps_the_hysteresis`
- R151 第 8 項 — 判斷同一個操作時加上提出者比對（AI-455）— `decision::tests::a_controller_gate_neither_joins_nor_consumes_requests_raised_by_a_worker`
- R151 第 9 項 — Controller 取消預算已用盡的 Task 前，要先送 NextStepAfterBudget 裁定（AI-457）— harness `retry_budgets::the_controller_cannot_cancel_a_task_whose_budget_is_exhausted_without_a_decision`
- R151 第 10 項 — 預算用盡的裁定請求不因契約改版而過期（AI-456）— `decision::tests::the_budget_exhausted_request_survives_a_contract_change_…`
- R151 第 11 項 — Gate 重驗排除 reproduce／verify_fix 用途的驗證要求 — harness `exploration_to_reproduction::a_queued_reproduction_does_not_stand_in_for_…`
- R151 第 12 項 — Task 已完成或已取消時，依據被推翻也不讓 Workflow 退回（AI-458）— harness `gate_invalidation::an_overturned_assumption_does_not_strand_…`
- R151 第 13 項 — 審查的 candidate 與目前 build 不同時判 BuildChanged（AI-459）— harness `two_axis_review::a_review_of_an_older_candidate_cannot_open_the_gate_…`、store `reviews::tests::a_candidate_matches_…`
- R151 第 14 項 — 回歸案例只在受驗目錄包含修復時執行（AI-451）— store `exploration::tests::regression_cases_only_run_on_subjects_that_contain_the_fix`
- R151 第 15、18 項，review-notes #5 第一點 — 還原時：把備份當下那次啟動記為正常結束；schema 比程式新就拒絕；manifest 的成果路徑不能跑出還原目錄，雜湊格式也要正確（AI-473）— `backup::tests::first_start_after_restoring_…`、`a_backup_from_a_newer_schema_is_not_restored`、`manifest_paths_that_escape_the_restore_target_are_rejected`
- R151 第 16 項 — `read()` 包在讀取交易中，同一個查詢讀到同一個快照 — `tests::one_read_sees_a_single_snapshot_even_if_a_write_commits_in_between`
- R151 第 17 項 — 秘密偵測涵蓋 `--token 值` 與 args 陣列中相鄰的旗標和值 — `shared_settings::tests::a_secret_named_flag_followed_by_its_value_…`
- R151 第 19 項 — 試寫提交成功後，在寫入佇列裡清除故障 — `health::tests::a_fault_recorded_by_the_next_queued_write_survives_…`
- R151 第 20、21 項 — BeginLaunch 重新檢查額度門檻；QuotaLimited 會重新排隊（AI-464）— `run::tests::the_launch_intent_rechecks_the_accounts_quota_gate_…`、`a_quota_limited_exit_requeues_…`、store `runs::tests::a_launch_intent_on_an_account_whose_quota_gate_closed_…`
- R151 第 22 項 — MarkReady 只接受 pending 與 blocked（AI-465）— `task::tests::only_pending_and_blocked_tasks_can_be_marked_ready`
- R151 第 23 項、review-notes #25 第一點 — build 與 revision 都觀察不到時不沿用舊結果；已送出的外部操作不重送（AI-462）— `verification_attempt::tests::a_completed_journey_is_not_reused_when_neither_…`、`…_does_not_resend_its_external_operation`
- R151 第 24 項 — 整合項目的 Run 已不是受驗的 Run 時直接失敗（AI-468）— `integration::tests::an_entry_whose_run_is_no_longer_the_task_subject_…`
- R151 第 25 項 — 新條目的範圍不得比被取代者窄（AI-472）— `knowledge::tests::a_replacement_cannot_have_a_narrower_scope_…`
- R151 第 26 項 — 回報重犯時，教訓的範圍必須涵蓋該 Task — `guard::tests::a_recurrence_cannot_be_reported_on_a_task_the_lesson_does_not_cover`
- R151 第 27 項 — begin_controller_run 在交易內拒絕已封存的 Workspace — `conversations::tests::a_controller_turn_is_not_begun_once_the_workspace_is_archived`
- R151 第 28 項（部分修正）— 確認授權可帶選填的 expected_revision，不符時回 409（AI-471）— `authorization::tests::a_confirmation_based_on_a_stale_revision_is_rejected`
- R151 第 29 項、review-notes #9 第二點 — 區段標題改成繁體中文；移除授權說明與 `RUN_COLUMNS`／`RUN_COLUMNS_QUALIFIED` 字串中多餘的空白 — 純文字修改
- review-notes #11 — Worker Run 回報的原生識別已被其他 Session 綁定時（`InUse`），一律不綁定，並要求這個 Run 停止（AI-466）— `run::tests::a_run_reporting_a_native_session_owned_by_a_suspended_session_…`
- review-notes #12 第一點 — 註解移回 failed_check 上方 — 純註解修改
- review-notes #17 — 時間線改在 SQL 端依 Workspace 篩選；裁定請求關聯到它阻擋的每個 Task — `external_operations::tests::a_decision_blocking_several_tasks_appears_on_each_blocked_tasks_timeline`
- review-notes #20 — Stage 預算用盡後，只能經使用者裁定才能繼續（AI-460）— harness `methods_playbooks_stages::a_stage_whose_budget_is_used_up_only_continues_after_the_user_decides`
- review-notes #21 第一點 — Stage 證據只接受 Workflow 綁定的 Task（AI-461）— harness `gate_invalidation::verification_and_acceptance_evidence_must_belong_to_…`
- review-notes #22 — 在 Domain 端先還原 git 的引號路徑再比對，無法還原就視為受影響（AI-470）— `review::tests::git_quoted_paths_are_unquoted_…`
- review-notes #26 — 含 `{{run_id}}`／`{{state_dir}}` 的步驟不重播也不錄製；只把驗證開始前就有的錄製標為 stale（AI-469）— `replay::tests::an_act_step_that_uses_run_scoped_placeholders_…`、`verification_profiles::tests::a_verification_cannot_mark_stale_a_recording_made_after_it_started`

NOT_FIXED:
- review-notes #10（查證後只重驗、把等待裁定中的 Task 加入阻擋範圍）：維持重新派工（AI-463）。第 1 項修好後，這條路徑不會再卡死，也補了 store 測試。
- review-notes #12 第二點（重驗加延遲）：維持 AI-274（AI-467）。
- R151 第 28 項的 Web 部分：Web UI 還沒送出 `expected_revision`（AI-471）。在 Web 改好之前，這項保護實際上不會生效。
- R151 第 25 項的「被取代條目也要帶 revision」：不實作（AI-472）。
- review-notes #11 的 Controller 路徑：Controller Run 沒有取消機制，這個邊界留作剩餘風險（AI-466）。
- review-notes #15、#21 第二點、#25 第二點：主要程式在 guardian／runtime，屬 R155 範圍，沒有處理。

NOTES:
- **範圍外的修改**：
  - `crates/baton-api/src/authorization.rs`、`src-tauri/src/main.rs`、`crates/baton-client/tests/authorization_client.rs`：轉送選填的 `expected_revision`。
  - `crates/baton-api/src/controller.rs`：`cancel_task` 加上裁定 Gate（第 9 項）。
  - harness 測試只新增情境：`worktrees.rs`、`retry_budgets.rs`、`exploration_to_reproduction.rs`、`gate_invalidation.rs`、`two_axis_review.rs`、`methods_playbooks_stages.rs`。
  - `verification_handoff.rs` 的 `world()` 給受驗目錄設定 revision "c1"。原因是第 23 項之後，兩者都觀察不到就不再沿用；這個檔案的斷言沒有改。
- **需要整合時處理**：
  - 新增 migration 46 `verification_pass_per_run`。
  - AI 編號用到 449～473，可能與 R155、R156 撞號。
  - Web 的確認呼叫要帶上 `expected_revision: summary.changes?.base_revision` 才會生效；這次沒改，是因為 R156 正在改 `AuthorizationPanel.tsx`，避免衝突。
- **改過預期的既有測試**：`workflow::tests::failures_follow_the_failure_route_until_the_budget_is_used_up` 最後一段原本預期「預算用盡後附齊證據就繼續」，與 review-notes #20 要的行為相反，已改成沒有核准就維持 blocked。其他斷言保留。
- **行為變化，請驗證者特別看**：
  - 第 13 項之後，reviewer 必須填被審 commit 的 SHA；填分支名稱的審查開不了 Gate。
  - 第 8 項之後，Controller 對同一事項會另建自己的裁定請求，不再沿用 Worker 或 Runtime 建立的。
  - 第 7 項之後，等待資源的交接不再拿推估的重置時間排程，改走退避。
  - 第 3 項的新限制還沒寫進 mcp-shim 的工具說明，目前只靠錯誤訊息告知 Controller。
- 子代理 B 回報：`verification_handoff` 有 2 個測試在機器負載高時失敗過一次，單獨重跑都通過，判斷是時序不穩。最終的 `pnpm test` 全部通過。
- 子代理用的 worktree 已自動清除；它們的 commit 都已併入本分支。
