```
RESULT: DONE
COMMITS: e7697ad..933bd0e
TESTS:
- `pnpm test` 會超過前景 10 分鐘的上限，所以把 scripts/test.mjs 的每一步分開執行，全部通過：
  - `cargo clippy --workspace --all-targets -- -D warnings`：無警告
  - `cargo test --workspace --exclude baton-desktop --exclude baton-harness`：通過
  - `cargo test -p baton-harness --lib --test <68 個測試檔>`（分兩批）：通過
  - `pnpm --filter web test`：30 檔、678 項通過
  - `node scripts/test-e2e.mjs`（BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw）：4 項通過
- `pnpm --filter web typecheck`：通過
- `cargo check -p baton-desktop`：通過
- 以上是最後一次 `git merge cli/v1`（已含 fix-c）之後跑的結果，工作樹乾淨。
- 新增的 Controller CLI 情境測試各重跑 5～6 次，並以 3 份同時跑加壓 3 輪，全部通過。
- lifecycle 修正：子代理加壓 80 次以上全部通過，修正前的新測試 5/5 失敗。
FIXED:
- R026 第 1 項 — 換帳號後要不要開新 Run，改由 Store 依交易內讀到的 Run 決定。apply_cli 加 next 參數（end_cli 併入），Supervisor 一律備妥新 Run 的識別 — harness `a_switch_requested_before_*` 兩項
- R026 第 2 項 — `begin_launch` 遇到 `end_requested_at` 有值時直接以 user_ended／switched 結束，不寫 launch intent；Supervisor 依快照的檢查移除 — Domain `a_run_whose_end_was_requested_before_the_launch_intent_*`、harness `a_switch_requested_before_the_window_opened_*`
- R026 第 3 項 — 還原的同一個交易中，非終態的 kind='cli' Run 以 restored_from_backup 中斷 — harness `after_restore_an_unfinished_controller_cli_is_not_opened`
- R026 第 4 項 — 新增 `controller_credential_valid`，`controller_run_active` 與 messages 的 Controller 寄件者共用 Domain 的 `credential_valid` — store `only_an_open_controller_cli_that_was_not_rebound_may_send_as_the_controller`
- R026 第 5 項 — open 遇到結束中的 Run 回 Ending；換帳號遇到結束中回新的 SwitchWhileEnding，代碼不變 — Domain `opening_while_the_previous_run_is_ending_*`
- R026 第 6 項 — `ai_decisions_path()` 改由 `AI_DECISIONS_RELATIVE_PATH` 組成路徑 — 既有路徑測試與手冊測試
- R027 第 1 項 — shim 的工具說明與 API 回給 Controller 的訊息改寫成 wait／「裁定已回答」／「Controller CLI 已結束或已重新綁定」，只改文字、不改行為 — 沒有測試斷言舊文字
- R027 第 2 項 — 完全退出時 CLI 的停止改用 FuturesUnordered，與 Worker 合成同一個串流逐筆記錄 — harness `full_exit_reports_each_controller_cli_as_it_stops_even_when_another_hangs`
- R027 第 3 項 — `create_file_if_absent` 寫入或 fsync 失敗時刪掉剛建立的檔案 — workspace_fs 注入寫入失敗與已存在兩項
- R027 第 4 項 — `understanding.rs`（api 與 store）的模組文件改為「由使用者在知識頁簽協調」
- review-notes 第 3、5 項（同一支 lifecycle 測試）— 根本原因是外部程式以不允許刪除共享的方式短暫開著目標檔，rename 與讀取都會回 os error 5；改為依錯誤種類有上限地重試 — 新加壓測試 `writers_and_readers_survive_other_programs_briefly_opening_the_lifecycle_file`
- review-notes 第 6 項 — 新增 Port 方法 `AgentAdapter::prepare_controller`，開 CLI 時只寫 mcp.json — harness `the_run_directory_gets_the_mcp_config_but_no_worker_files`、adapter `preparing_a_controller_cli_writes_only_the_mcp_config_*`
- review-notes 第 7 項 — 刪除 notices／ConversationNotice／WorkflowNote／SessionOwner::Conversation，修正「喚醒 Controller」等註解 — 只刪了斷言這些輸出的部分
- review-notes 第 9 項 — 刪除沒有呼叫端的 `Store::controller_cli_accounts`
- review-notes 第 10 項 — Runtime 重啟後中斷舊 Run 時，若已要求換帳號，同一個交易中以新帳號開新 Run — harness `a_switch_requested_before_a_crash_still_opens_the_new_account_when_the_host_is_gone`
NOT_FIXED:
- R027 第 2 項的後半「CLI 的停止提前到 Worker 的 find 迴圈之前開始」沒有做。主要缺陷（整批結果被丟掉）已修正；要提前開始得重排整段停止流程，風險大於收益（AI-58）。
NOTES:
- AI 決策編號：R030 已用 AI-50～52，所以本 run 的條目改編為 AI-53～AI-62，並在合併 cli/v1 後移到 AI-52 之後（內容不變）。R032-fix-d 若也從「最大號」接續，整合時會再和 AI-53 以後的編號撞號，主控合併時要留意。
- 跨範圍修改：`crates/baton-harness/tests` 新增或修改了情境測試（controller_cli、controller_cli_accounts、controller_cli_exit_and_recovery、backup_and_restore），沒有改 harness/src。另外 `.scratch/.../ai-decisions.md` 追加了條目。
- 需要驗證者特別看：
  - Store 的 `apply_cli`：Run 進入終態、而交易內讀到的 Run 有 `switch_to_profile_id` 時，一律開新 Run。完全退出仍傳 None，不開新 Run。
  - 換帳號後經過當掉，舊 Run 是 interrupted，同時會開出新 Run（AI-53）。
  - 還原報告的 `interrupted_runs` 現在也算進 Controller CLI（AI-54）。
  - Controller 寄件者若是舊輪次（kind='turn'），messages 改回 RunNotFound（AI-55）。
- 本 run 中途觀察到一次 `baton-client/tests/shared_settings_client.rs` 的 `app_gets_a_run_settings_snapshot_*` 失敗（第 129 行 unwrap）。當時機器上同時在跑子代理的加壓測試；之後單獨重跑 6 次、整包重跑 2 次都通過，未追查成因，建議記入 review-notes 留意。
- 範圍外但有同類風險（子代理回報，未處理，已記在 AI-61 的「影響」）：
  - `crates/baton-guardian/src/files.rs` 的生命週期檔讀寫沒有重試，而且用固定的 `.tmp` 檔名。
  - `baton-credential`、`backup.rs`、`update_source.rs` 直接用 `fs::rename`。
- 兩個子代理的 worktree（`issue-orchestrator/.claude/worktrees/agent-a81231db13e60c5d8`、`agent-a7bc0a2d6e78cef1e`）及其分支都已合併進 `cli/final-ab`，尚未刪除，可清理。
- 其他 crate 的程式碼註解（不會送進模型）還有零星的「一輪」用語，子代理已列出位置，沒有全面改寫。
```
exit: 0

