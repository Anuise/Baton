VERDICT: PASS

CRITERIA:
- [PASS] 驗證中途交接後結果正確，未完成案例不會被拼成 passed — 以旅程完整結束作為交接邊界。只有旅程完整結束時才寫入 `JourneyCompleted`，因交接中止的旅程不寫（`crates/baton-runtime/src/verification_adapter.rs:498-504`）。`plan_journey` 只沿用「已完整結束、而且 build 與 revision 都相同」的旅程，其餘整條重跑（`crates/baton-domain/src/verification_attempt.rs` 的 `plan_journey`）。承接者的 `run.steps` 只放三種紀錄：這次自己的步驟、沿用的旅程、不重送的旅程，前一次做到一半的步驟不會放進結果。交接前會先 teardown、停止服務、關閉 Session，之後才回傳 `HandedOff`（`verification_adapter.rs:829-856`）。承接者先用日誌裡的參照停止前一次留下的程序（`take_over`）；無法確認已停止時判為 `environment_blocked/previous_session_unconfirmed`。正式 driver 的識別帶有 Runtime 實例標記，不是這個 Runtime 啟動的識別一律回報「無法確認」（`verification_driver.rs`、`browser_driver.rs`）。以下情境測試都通過：
  - `a_rate_limited_judge_hands_off_and_the_successor_reruns_the_unfinished_case_from_a_fresh_fixture`
  - `an_unfinished_case_is_not_stitched_into_a_pass_when_the_successor_cannot_finish_it`
  - `after_a_crash_the_successor_stops_the_old_test_processes_and_reruns_the_unfinished_case`：第一次嘗試中「開啟」已通過，承接後重跑出現 product_failure，沒有被拼成通過
  - `a_successor_runs_nothing_while_an_old_test_process_cannot_be_confirmed_stopped`
- [PASS] 結果未知的外部操作不因重測而重送 — `run_step` 會在送出前先寫入 `ExternalStarted`，寫不進去就不送出，該步驟記為 `journal_unavailable`（`verification_adapter.rs:235-241`）。若未完成的旅程裡有已開始的外部操作，承接時判為 `NotResent`：既不重送，也不重跑那條旅程，結果固定為 inconclusive（`not_resent_result`）。`external` 只能用在旅程步驟，這點在 Profile 驗證時就會檢查（`verification_profile.rs` 的 `check_external_only_in_journeys`）。以下測試都通過：
  - `an_external_operation_with_an_unknown_result_is_not_resent_after_a_crash`：`POST /orders` 只送出 1 次；再處理一次仍是 1 次
  - `a_case_that_already_sent_an_external_operation_is_not_rerun_after_a_handoff`
- [PASS] 以 Runtime harness（spec D-04）撰寫的情境測試涵蓋本票行為並通過，AI 決策也已追加 — `crates/baton-harness/tests/verification_handoff.rs` 共 7 個測試，都經過真實 HTTP API 執行，結果 `7 passed; 0 failed`。其中也包含帳號額度與全帳號受限的情境：`the_judge_only_uses_accounts_below_the_quota_gate_and_without_any_only_model_free_assertions_run`。`ai-decisions.md` 已追加 AI-425～AI-430，每條都標了重要度（高／中／高／高／高／低）。

前置條件：驗證開始時 `rebuild/v1`（e012c0b）是 HEAD 的祖先，`merge-base --is-ancestor` 回傳 ANCESTOR_OK。驗證途中整合分支前進到 4e51789，多了一個 commit（`chore(scratch): 記錄 R143，準備 R146`，只改 scratch 紀錄），所以現在已不是祖先。依指示只在這裡標註，不據此判 FAIL。

TESTS:
- `cargo test --workspace --exclude baton-desktop --no-run`（經 with-msvc.ps1）→ EXIT=0
- `pnpm test`（`node scripts/test.mjs`）→ EXIT=0
  - Rust：1610 passed、0 failed、8 ignored
  - 前端 vitest：29 個檔案、618 個測試全部通過
- `cargo test -p baton-harness --test verification_handoff` → 7 passed
- `BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw cargo test -p baton-runtime --test browser_driver` → 3 passed，包含真實瀏覽器測試 `a_session_drives_a_real_browser_and_returns_its_trace_and_video`。指示中的路徑 `Tempaton-pw` 我判斷是 `Temp\baton-pw` 少了反斜線，該目錄確實存在。
- `pnpm build`（`node scripts/build.mjs`）→ EXIT=0，產出 `target/release/baton-desktop.exe`。建置過程只有一個 warning：`crates/baton-api/src/shared_settings.rs:20` 有未使用的 import `StoreError`，這個檔案不在本票的 diff 裡。
- 驗證結束後 `git status` 是乾淨的，沒有改動任何追蹤中的檔案。

FIXES_NEEDED: none

以下兩點不影響判定，給後續參考：
1. `plan_journey` 只在「未完成」的旅程裡找已開始的外部操作。如果某條旅程在前一次嘗試已完整結束、裡面也有外部操作，但承接時 build 或 revision 不同（例如 build 識別的輸出含時間戳），這條旅程會整條重跑，外部操作會再送一次。這個操作的結果已知，所以不算違反驗收條件，但與 AI-428「已開始的不重送」的說法不完全一致。
2. 承接者無法確認舊程序已停止（blocked）時，仍會執行 `prepare_state_dir`、teardown 和 `remove_state_dir`。AI-429 寫的是「不執行任何操作」，說法比實際行為強；實際上沒有啟動新服務，也沒有重跑旅程。
