VERDICT: PASS
CHECKED:
- [PASS] R152 第 1 項【高】：Runtime 重啟後只剩子孫程序時，`find`／`stop` 的身分判定 — 修法分三部分：
  - 根程序現在會繼承一個只能查詢的 Job handle，Runtime 關閉自己的 handle 之後，具名 Job 仍找得回（`windows_process.rs:430`）。
  - `key_for` 會把資料庫記下的根身分對應到 `find` 找回的登記。
  - Supervisor 在 `Exited` 和 lease 核對判定 Dead 時都會先呼叫 `release`，無法確認就不結束 Run（`supervisor.rs:1251`、`:2208`）。

  我在 repo 外的 `git archive` 副本做了變異測試，確認新測試能抓到缺陷：
  - M1 拿掉 `key_for` 的對應：`after_a_restart_the_leftovers_of_a_dead_root_are_still_stopped_by_the_recorded_identity` 失敗，`stop` 回 StoppedGracefully，不是 ForceKilled。
  - M2 不讓根程序繼承 Job handle：上面那個測試和 `find_resumes_control_after_the_spawning_instance_is_gone` 都失敗。這也證實了實作者說的「修正前 Runtime 重建後 find 一律失效」。
  - M3 拿掉 Worker 結束時的 release：真實 Job 的 `a_finished_worker_leaves_nothing_running_in_its_job` 和 harness 的 `a_worker_that_exits_ends_its_run_only_after…` 失敗。
  - M4 拿掉 lease 核對時的 release：`an_expired_lease_with_a_dead_root_is_concluded_only_after_the_leftovers_are_released` 失敗。
- [PASS] R152 第 2 項【中】：取消時告知 Worker，多個取消並行停止 — `agent.rs:189` 加入 `cancel_requested_at` 判斷；`process_cancellations` 改用 FuturesUnordered。M5 拿掉取消判斷後，`a_worker_is_told_to_save_and_stop_once_its_run_is_cancelled` 失敗。另有並行停止的測試 `cancelled_runs_are_stopped_side_by_side…`。
- [PASS] R152 第 3 項【中】：npm 的 `claude.cmd` — Worker、Controller、語意 judge、`auth status` 和 `--version` 探測都改用同一個 `claude_program()`，Worker 與 Controller 的 Adapter 只剩這一條啟動路徑。批次檔經 `cmd.exe /d /v:off /c call` 啟動，並拒絕 cmd 會重新解讀的字元。有 `resolve_program` 單元測試，也有經真實 Job 啟動模擬 `claude.cmd`（路徑含空白）的測試。
- [PASS] R152 第 4 項【中】：已結束 Run 的串流與暫存表會清掉 — `pump_events` 開頭對三個表做 retain。M6 拿掉之後，`a_run_ended_by_lease_reconciliation_no_longer_holds_its_event_stream` 失敗。
- [PASS] R152 第 5～11 項【低】：都逐一讀過 diff。
  - 第 5 項（semver 字元檢查）、第 7 項（驗證命令放進 KillOnClose Job 並分開讀輸出）、第 8 項（`blocking()`）、第 9 項（兩處補 `\` 續行）、第 10 項（共用 `reqwest::Client`）、第 11 項（`OpenProcess` 依錯誤碼判斷）的修法都和審查建議一致。
  - 第 6 項（恢復鏡像）做了變異測試 M7，`a_recovery_mirror_that_failed_to_write_is_written_again_on_the_next_pass` 會失敗。
- [PASS] review-notes 本範圍的項目：#3、#4、#7、#8、#9 第一點、#13、#14、#15、#16（部分）、#18 第二點、#19、#22、#24、#25 第二點、#29 第二點都有對應修正與測試。
  - #1、#5 第一點、#10、#11、#12、#17、#20、#21 第一點、#25 第一點、#26 我核對過 R154，確實由修正 A 處理；#23、#28 由修正 C 處理，web 相關不在本範圍。
- [PASS] NOT_FIXED 的理由都合理，也都有 AI 條目：#6(b) 對 AI-479、#16 其餘部分對 AI-488、#18 第一點與 #21 第二點對 AI-489、#27 第二點對 AI-487。
  - 已知殘餘風險：子孫若以不繼承 handle 的方式建立，Runtime 重建後退回以 PID 判斷。這點已寫進 AI-475 的「影響」。Node 和 libuv 建立子程序時會繼承 handle，實際情境都在保護範圍內。
- [PASS] 沒有刪除或弱化既有測試。被刪的測試行只有三類：
  - `guard::report` 簽名改變後的呼叫更新，斷言不變，另外補了只帶 Workspace 的案例。
  - `guardian.rs` 改用非空目錄佔住生命週期檔來注入寫入失敗，意圖不變。
  - `supervisor_process_tree.rs` 抽出共用 helper，斷言都保留。原本「至少 3 個程序」的等待條件改成「等 ping 出現」，等待更嚴格。
- [PASS] 沒有發現新缺陷。`InheritList` 的 unsafe 部分我看過：重複的 stdout／stderr handle 會先 dedup，清單在 `CreateProcessW` 之後才釋放。新增的已知取捨有兩個：Worker 結束時在 tick 鎖內最多多等 5 秒（已記入 AI-475）；`find` 出錯時仍退回以 PID 判斷，只在 Job 裡所有程序都開不了的極端情況下才有影響。
- [PASS] 前置條件：驗證途中 `rebuild/v1` 前進到 ecbbc65，但只有 `.scratch/` 的紀錄（R159、R161 的提示檔），沒有程式碼變更，不影響結論。工作樹全程乾淨，變異測試的暫存副本已刪除。

TESTS:
- `pnpm test`（設 `BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw`）：通過，結束碼 0，約 6 分鐘。
  - Rust：1746 項通過、0 失敗、8 項 ignored（需要已登入 Claude 的 live 測試）。
  - 前端：30 個檔案、636 項通過。
- 用 `--nocapture` 另外確認真實瀏覽器測試有實際執行：`web_verification_secret_fields` 有跑，`browser_driver` 4 項通過。唯一的「略過」是刻意測試「設了 BATON_PLAYWRIGHT_DIR」情境的那一項。
- `pnpm --filter web typecheck`：通過。
- `cargo check -p baton-desktop`：通過。
- 變異測試 M1～M7（在 repo 外的副本）：每個變異都讓預期的新測試失敗。
- 任務說明裡的 `%LOCALAPPDATA%\Tempaton-pw` 看起來是 `\b` 被吃掉的結果，實際存在且已使用的是 `%LOCALAPPDATA%\Temp\baton-pw`。

FIXES_NEEDED: none

給主控的觀察（不影響判定）：
1. 決策編號衝突：`rebuild/final-fix-c` 和本分支都用了 AI-475～AI-484，兩者都還沒合併進 `rebuild/v1`。後合併的一方要順延編號，並同時處理 `supervisor_process_tree.rs` 和 `command_runner.rs` 可能出現的合併衝突（R159 也改過這兩個測試檔）。
2. AI-489 少了 `- 審閱：未審` 這一行，和 AI-490 之間也少一個空行，屬於格式小問題。
3. AI-488 的 Controller 共同工具缺口（`send_message`、`read_inbox`、`accept_artifact`、Controller 版 `record_lesson_candidate`）需要另開票，或在 spec D-05 加註記，這需要主控決定。
