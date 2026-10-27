RESULT: DONE
COMMITS: 1aed180..4b58300
TESTS:
- `pnpm test`：通過。Rust 為 `cargo test --workspace --exclude baton-desktop` 全數通過；前端 30 個測試檔、636 項通過。有設 `BATON_PLAYWRIGHT_DIR`，所以真實瀏覽器測試有實際執行。
- `pnpm --filter web typecheck`：通過。
- `cargo check -p baton-desktop`：通過。
- 這三項是在 1e8f815 上跑的。最後一個合併 4b58300 只動到 `.scratch/`，程式碼與 1e8f815 相同。
- 另外單獨跑過：`process_tree` 連跑 3 次；`duplicate_command_id_is_rejected` 連跑 40 次，0 失敗。
- 工作樹乾淨，`HEAD..rebuild/v1` 沒有新的 commit。

FIXED:
- R152 第 1 項【高】，以及 review-notes #14 — 程序樹收尾 — `run_process_release.rs`、`process_tree.rs` 新增的 4 個測試、`supervisor_process_tree.rs::a_finished_worker_leaves_nothing_running_in_its_job`
  - 修正時實測發現更根本的問題：Runtime 關閉自己的 Job handle（包括崩潰）之後，具名 Job 就找不到了，即使 Job 裡還有程序。也就是說，原本 Runtime 重建後的 `find` 一律失效。
  - 修法：根程序繼承一個只能查詢的 Job handle，並用 handle list 限定子程序只繼承必要的 handle。
  - `find` 以 Job 對應資料庫記下的根身分；新增 `ProcessControl::release`。
  - Supervisor 在 Worker 結束（包括交接轉移）或 lease 核對確認根程序不在時，先 `release`；確認不了就不結束 Run。
- R152 第 2 項 — 取消 Run 後 Worker 的回報回應帶 `stop_requested`；多個取消同時停止 — `run_process_release.rs` 2 個測試
- R152 第 3 項 — npm 安裝的 `claude.cmd` 經 `cmd.exe /d /v:off /c call` 啟動，並拒絕會被 cmd 重新解讀的引數 — `claude_cli`／`claude_adapter`／`semantic_judge` 的 6 個測試
- R152 第 4 項 — 已結束 Run 的事件串流、最近錯誤、限流訊號在下一次處理時清除 — `a_run_ended_by_lease_reconciliation_no_longer_holds_its_event_stream`
- R152 第 5 項 — 更新清單的 `version` 只接受 semver 字元 — `a_version_that_could_escape_the_download_directory_is_rejected`
- R152 第 6 項 — 恢復鏡像只有寫入成功才記為已鏡像 — `a_recovery_mirror_that_failed_to_write_is_written_again_on_the_next_pass`
- R152 第 7 項與 review-notes #3 — 驗證命令放進 Job Object，等命令本身結束，逾時時保留已讀到的輸出；連結指向工作目錄外的檔案判為不通過 — `command_runner.rs` 2 個測試，另有 junction 與 file-check 測試
- R152 第 8 項 — `write_readonly_files` 包進 `blocking()` — 純結構修正，沒有加測試
- R152 第 9 項與 review-notes #8 — `agent.rs` 兩則訊息補上 `\` 續行 — 純文字修正
- R152 第 10 項 — `RuntimeClient` 共用一個 `reqwest::Client` — `consecutive_requests_from_one_client_reuse_the_connection`
- R152 第 11 項與 review-notes #15 第二點 — 監護的 `OpenProcess` 失敗時，只有程序不存在才算已結束 — `a_process_this_user_cannot_open_is_not_assumed_gone` 等
- review-notes #2 — 兩個偶發失敗測試的根因
  - `artifacts_client`：派工請求被套用一般請求的 2 秒上限，改用 60 秒上限。同時跑 400 份比較，修前 111 次失敗，修後 0 次。測試 `dispatch_waits_beyond_the_plain_request_limit_while_the_runtime_prepares_the_workdir`。
  - `process_tree`：Job 內還有 `conhost.exe`，「至少 3 個程序」可能在 ping 啟動前就成立。改成等 ping 確實出現。
- review-notes #4
  - 完全退出時找回程序也受總上限約束 — `full_exit_keeps_its_deadline_even_when_finding_a_process_hangs`
  - 生命週期檔每次寫入用自己的暫存檔 — `concurrent_writers_of_the_lifecycle_file_do_not_trip_over_each_other`
- review-notes #5 第二點 — `restore` 失敗訊息改以「還原失敗：」開頭 — `restore_command.rs`
- review-notes #6(a) — 快照檔參數改由 `RunSpec.settings_files` 決定，不再用 `is_file()` 判斷 — `the_snapshot_files_passed_come_from_the_run_spec_and_not_from_the_file_system`
- review-notes #7 — 重犯回報只帶 `workspace_id` 時保存 Workspace — `a_recurrence_reported_with_only_a_workspace_keeps_the_workspace`
- review-notes #9 第一點 — 移除未使用的 `StoreError` import
- review-notes #13 — 主控台 Ctrl-C 與服務自行結束都走正常 `shutdown()` — `runtime_interrupt.rs`
- review-notes #15 第一點 — 監護日誌加鎖，Runtime 在鎖內讀取 — `a_runtime_importing_during_the_final_check_never_sees_an_attempt_without_its_withdrawal`
- review-notes #16（部分）— Controller 可以用 `search_knowledge` — `controller_shared_tools.rs`
- review-notes #18 第二點 — 補 M7 情境測試 — `controller_handoff_single_account.rs`
- review-notes #19 — 偏好關閉時，依值名稱移除舊的登入啟動登記 — `login_startup_stale_registration.rs`
- review-notes #22 — 變更路徑改用 `git diff -z` — `changed_paths_returns_names_with_quotes_tabs_and_non_ascii_characters_verbatim`
- review-notes #24 — 秘密偵測也讀輸入欄位的值，trace 改為不壓縮 — `browser_driver.rs`、`web_verification_secret_fields.rs`，都以真實瀏覽器測試
- review-notes #25 第二點 — 承接者無法確認舊程序已停止時，不碰狀態目錄也不 teardown — `verification_takeover_blocked.rs`
- review-notes #27 — 確認規劃限流時也交接是合理的，沒有改程式（見 AI-487）
- review-notes #29 第二點 — mcp-shim 的 `reproduce_exploration_finding` 說明寫入 R151 第 3 項的案例限制 — 純文字修正
- 額外發現 — mcp-shim 的 `record_lesson_candidate` 描述中夾帶一長串空白，已刪除

NOT_FIXED:
- review-notes #6(b) 每次啟動前比對 CLI 版本：不做（AI-479）。
- review-notes #16 的其餘部分：Controller 的 `send_message`、`read_inbox`、`accept_artifact`、Controller 版 `record_lesson_candidate` 都需要新的 Domain／Store 模型，留待另開票（AI-488）。
- review-notes #18 第一點：`active_runs: &[]` 與 AI-305 一致，實際喚起下一輪時也用同樣的輸入，不會發生「停掉舊輪才發現放不下」（AI-489）。
- review-notes #21 第二點：驗證與整合共用 `verify_lock`，維持不變（AI-489）。
- review-notes #27 的用量與動作只計最後一次嘗試：不修（AI-487）。
- 不在本範圍、沒有處理：review-notes #2 的 web 測試、#10、#11、#12、#17、#20、#21 第一點、#23、#26、#28、#29 第一點。其中 #10、#11、#12、#17、#20、#21 第一點、#26 已由修正 A 處理。

NOTES:
- **決策編號**：修正 A 佔用了 AI-449～AI-473，主控又新增 AI-474，本 run 的條目最後是 AI-475～AI-490。commit aabd0dc 與 1e8f815 的訊息裡寫的編號是改號前的。
- **跨範圍修改**
  - `baton-store/src/lifecycle.rs`：暫存檔名，兩行。
  - `baton-store/src/guards.rs` 與 `baton-domain/src/guard.rs`：#7 需要。合併時與修正 A 的範圍檢查都保留，並把修正 A 新增的 guard 測試改成新的函式簽名。
  - `baton-domain`：新增 `Observation::FileOutsideWorkdir`。
  - `baton-harness/src/fakes.rs`：只新增 `release`／`release_calls`、`hang_finds`、`live_streams`、`fields`、`leave_stale`。
  - `baton-harness/tests/`：新增 8 個測試檔。
  - `baton-harness/tests/guardian.rs`：一個既有測試原本靠固定的 `lifecycle.json.tmp` 注入寫入失敗，暫存檔名改成每次不同後失效，改成用非空目錄佔住生命週期檔本身來注入同樣的失敗。斷言改為 `!is_file()`，意圖不變。
  - `docs/development.md`：程序樹、驗證命令、`git diff -z`、重犯 API、Ctrl-C 的說明。
- **請驗證者特別看**
  - `windows_process.rs` 的 `InheritList`／`inheritable_copy`（unsafe）。
  - `key_for` 的對應規則：依賴「啟動指令雜湊每個 Run 不同」。
  - Worker 結束時最多多等 5 秒讓程序樹收尾。
  - Worker 的子孫若以不繼承 handle 的方式建立，根程序結束且 Runtime 重建後仍可能找不回 Job，這時退回以 PID 判斷。
- **既有條目已過時、不能改**：AI-196「不支援 claude.cmd」已由 AI-477 取代；修正 A 的 AI-450「工具說明尚未更新」已在本 run 處理。
- **子代理的 worktree 還留著**：在主 checkout 的 `.claude/worktrees/agent-*`，內容都已合併，含各自的 target 目錄，佔磁碟空間。分支是 `final-fix-b-sub-items`、`worktree-agent-a3243e76b30a5bde7`、`worktree-agent-aa55d8edb71514caa`。是否刪除由主控決定。
