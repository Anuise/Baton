VERDICT: FAIL

**結論：** 建置與全部測試都通過，驗收條件 1、2、4、5 也符合。但條件 3「40 秒內結束」只在完全退出開始時沒有別的停止動作進行中才成立。我用一個暫時的探測測試重現了失敗：若 Supervisor 當時正在停止一個「已取消」的 Run，退出會超過 40 秒，Port 卡住時甚至不會結束。條件 6 因此不完整。

CRITERIA:
- [PASS] 關窗後 Run 持續；重開視窗連回同一 Runtime，不重複啟動 Worker — `src-tauri/src/main.rs` 的 `on_window_event` 把 `CloseRequested` 改成 `prevent_close()` 加 `hide()`。系統匣有「開啟 Baton」與「完全退出」，狀態每 3 秒以 `TRAY_POLL` 更新。`crates/baton-client/tests/lifecycle_client.rs` 的 `closing_the_window_keeps_runs_going_and_reopening_connects_to_the_same_runtime_without_another_worker` 確認：`Connection::AlreadyRunning`、`boot_count` 不變、`spawn_count == 1`、沒有任何 stop 呼叫，Run 仍是 running。實際在 Windows 上點系統匣的行為沒有自動化測試，開發者已在 `docs/development.md` 標為 Validation Required。
- [PASS] 停止意圖生效後，佇列重試、額度恢復與交接都不能啟動新 Run — 有兩道阻擋：
  - 派工時：`scheduler::place` 先檢查 `Capacity.halt`（`crates/baton-domain/src/scheduler.rs`），派工與佇列處理都經過 `halt_for` 帶入它（`crates/baton-store/src/scheduling.rs`）。
  - 啟動前：`crates/baton-store/src/runs.rs` 在 `BeginLaunch` 前檢查 `launch_halted_in`。
  - 測試：`crates/baton-harness/tests/lifecycle.rs:266` 涵蓋佇列重試與新派工，`:298` 涵蓋額度恢復；交接由 `scheduler/tests.rs` 的 `stop_intent_halts_every_placement_including_handoff_and_quota_recovery` 涵蓋。Runtime 目前還沒有實際的交接路徑（票 28），只有 Domain 層的保證。
- [FAIL] 退出流程在 40 秒內結束，無法確認停止的程序被記錄為停止異常 — 停止異常的記錄是對的（`lifecycle.rs:340` 測試涵蓋 `still_alive`、`stop_unconfirmed`、`stop_failed`）。問題在時限：
  - `Supervisor::full_exit` 先等 `tick_lock`（`crates/baton-runtime/src/supervisor.rs:209`），拿到鎖後才寫生命週期檔，並從那時才開始算 40 秒（`:241`），不是從 `requested_at` 算。
  - `process_cancellations` 持鎖時呼叫 `process_control.stop(&identity, self.stop_grace)`（`:542`），這個呼叫最長 30+10 秒，而且多個 Run 是依序處理。
  - 結果：真實 Port 下退出可能拖到約 80 秒以上，桌面殼 60 秒的後備會先把 App 關掉，使用者看不到停止異常；Port 卡住時退出永遠不會結束。AI-125 自己說「Runtime 不能把有界性完全交給 Port」，與這個行為矛盾。
  - 探測結果：上限縮為 300ms，讓一個已取消 Run 的停止呼叫卡住（`FakeStop::Hang`），3 秒後讀到 `PROBE phase="stopping" completed_at=null lifecycle_file_exists=false`。也就是停止流程沒結束，停止意圖也還沒寫進生命週期檔（spec D-25）。
- [PASS] Workspace A 開、B 關時，重開後只有 A 的合格工作自動恢復 — `lifecycle.rs:431` 測試：重開後 A 的 Task 變回 running，B 的仍是 ready，等待原因 `awaiting_resume`；按「繼續工作」後 B 才派出。設定預設關閉，核對完成前 `/resume` 回 409。另有 `:478` 測試確認先核對程序，`:530` 測試確認只有檔案有停止意圖時採較保守者。
- [PASS] 完全退出不會把 Task 標為取消或完成 — `lifecycle.rs:394` 測試：running 退回 ready，blocked 與 verifying 不變，資料庫裡沒有任何 cancelled 或 completed。AI-121 記錄了這個做法。
- [FAIL] 以主要 seam（Runtime harness）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加 — harness 有 11 個情境測試、client 有 2 個，全部通過；AI-121 到 AI-128 已追加。但沒有測試涵蓋「退出時另有停止動作佔著 Supervisor 的鎖」，也就是上面那個重現得出來的缺陷。另外 AI-127 少了「審閱：未審」一行，和 AI-128 之間也少一個空行（`.scratch/platform-rebuild-v1/ai-decisions.md:1171-1172`）。
- 其他：`rebuild/v1` 是 HEAD 的祖先（`git merge-base --is-ancestor rebuild/v1 HEAD` 輸出 ANCESTOR_OK）。

TESTS:
- `pnpm test`：結束碼 0。
  - Rust：51 組測試結果全部 ok，各測試二進位檔都是 0 failed，含 `lifecycle` 11 passed、`lifecycle_client` 2 passed、`baton_domain` 230 passed、`baton_store` 53 passed、`baton_runtime` lib 17 passed 與 3 個 ignored 的 live 測試。
  - 前端：vitest 14 個檔案、212 個測試全部通過。
- `pnpm build`：結束碼 0，產出 `target/release/baton-runtime.exe` 與 `target/release/baton-desktop.exe`。
- 暫時探測 `cargo test -p baton-harness --test zz_r047_probe`：重現出上面的時限缺陷。檔案已刪除，`git status` 乾淨。

FIXES_NEEDED:
1. 讓完全退出不受 `tick_lock` 拖延，從要求退出的那一刻就有 40 秒上限：
   - **(a) 立刻寫生命週期檔。** 在 `POST /api/v1/lifecycle/exit` 持久化資料庫之後，或在 `full_exit` 取鎖之前，就把停止意圖寫進生命週期檔，不要排在鎖後面。
   - **(b) 從持久化的期限算時間。** `full_exit` 的期限改從 `report.deadline_at` 減去目前時間換算，不要從取得鎖的時間算。
   - **(c) 等鎖要有上限。** 例如 `timeout_at(deadline, tick_lock.lock())`；或者 `process_cancellations` 呼叫 `ProcessControl::stop` 時不要持有 tick 鎖，並看到停止意圖就交給退出流程處理。到期限還拿不到結果的 Run（包括正在取消中的）記為 `stop_unconfirmed` 或 `still_alive`，退出照常進入 `stopped`。
2. 在 `crates/baton-harness/tests/lifecycle.rs` 新增情境測試：一個已取消 Run 的停止呼叫卡住（`FakeStop::Hang`）時要求完全退出，斷言三件事：
   - 縮短後的上限內進入 `stopped`；
   - 生命週期檔立即包含 `stop`；
   - 卡住的 Run 被記為停止異常，而且保留寫入所有權。
3. 在 `ai-decisions.md` 追加這次修正的決策並標記重要度，同時補上 AI-127 缺的「審閱：未審」一行與空行。
