VERDICT: PASS
CRITERIA:
- [PASS] 前置條件：`rebuild/v1` 是 HEAD 的祖先。開始驗證時成立（`git merge-base --is-ancestor rebuild/v1 HEAD` → ANCESTOR_OK，當時 v1 在 4e51789）。驗證途中整合分支前進到 `8143ebf`（票 49 結案），現在已不是祖先。依指示只標註，不據此判 FAIL。合併時會有衝突要處理：票 49 也用了 migration 43（`verification_attempts`）與 AI-425～AI-430，票 48 的 migration 43 `verified_action_replay`（`crates/baton-store/src/lib.rs:1657`）與 AI-425～AI-431 都要順延編號（AI-431 已預告這個風險）。
- [PASS] 未經斷言確認的動作不會被錄製。
  - 錄製條件：`replay::proven`（`crates/baton-domain/src/verification_profile/replay.rs`）要求步驟判定為 Passed，而且至少一個非 judge 斷言成立。`run_act` 只在 `ActionSource::Live && proven && shown == description` 時才寫入 `recorded`（`crates/baton-runtime/src/verification_adapter.rs` 的 `run_act`）。
  - 發布檢查：只有 judge 斷言的 `act` 步驟會被拒絕（`verification_profile.rs` 的 `check_step`）。
  - 情境測試 `only_actions_proven_by_a_later_assertion_are_recorded_and_replayed_for_real` 涵蓋：模型選錯按鈕時沒有錄製，判定是 `inconclusive/act_postcondition_failed`；選對時才錄製；重播時 Runner 實際收到 click 1 次，並重新檢查後置條件；只有 judge 斷言的 Profile 回 422。
- [PASS] stale 快取不沿用舊成功。
  - `replay::decide`：錄製是 Stale 就不重播；身分六個維度（case、goal、params、persona、engine、data）任一不同就標 `<維度>_changed`，也不重播。`replay::disproven` 在重播的動作失敗或非 judge 斷言不成立時標 `postcondition_failed`。stale 狀態以 `apply_replay_updates` 寫入，跟 Bundle 在同一個交易（`crates/baton-store/src/verification_profiles.rs`）。
  - 情境測試 `a_stale_recording_never_carries_an_old_success_forward` 涵蓋 engine、goal、data 改變：每次 Runner 的 click 都是 0，判定不是 passed；引擎改回原值後仍維持 stale。
  - 情境測試 `a_replay_whose_postcondition_fails_marks_the_recording_stale` 涵蓋：重播失敗後錄製變 stale，網站恢復正常後也不再重播。
- [PASS] no_live_model 下任何模型呼叫都被阻擋，且以 fake 模型計數驗證。
  - 第一道防線：Adapter 開始時把模型 Port 換成一律拒絕的 `NoLiveModel`（`verification_adapter.rs:786`）。
  - 第二道防線：`judge()` 與 `plan()` 在呼叫前都先檢查 `self.no_live_model`。
  - 用 grep 確認：正式碼中模型呼叫只經過 `self.model.judge` 和 `self.model.plan`（`verification_adapter.rs:351`、`:511`），沒有其他路徑。
  - 情境測試 `no_live_model_blocks_every_model_call_at_the_adapter` 以 `FakeSemanticJudge::calls()` 計數：沒有錄製時呼叫數是 0；有錄製時重播 1 次、呼叫數不變；`usage.blocked_model_calls` 正確；驗證要求、Bundle 與概況都記著 `no_live_model`。
- [PASS] 以 Runtime harness（D-04）寫的情境測試涵蓋本票行為並通過；AI 決策已追加到 ai-decisions.md。
  - `crates/baton-harness/tests/verified_action_replay.rs` 共 6 個情境測試，經真實 Axum API 與 fake Port 執行，6 個全部通過。除了上面幾條，也涵蓋「正式環境副作用預設不可重播」與「重播與 live 用量分開」。
  - `ai-decisions.md` 追加了 AI-425～AI-431，都標了重要度。
- [PASS] 「錄製的動作」解讀為目標導向動作 `act`，這個解讀符合票面與 spec。
  - 符合票面：快取身分包含「目標」，GLOSSARY 的 Replay 也說錄製的是「動作」，而 Profile 寫定的步驟本來就是固定操作，沒有錄製的意義。spec 使用者故事 72 也相符。
  - 符合 D-29：`act` 只存在於 web 的 BrowserAction（以 Playwright 為基礎），只能用在旅程步驟，沒有擴大 Adapter 的範圍。
  - 符合 D-30：規劃跟 judge 共用 `judge_args()`，沒有 `--model`（`semantic_judge.rs:46-61`），沿用帳號自己的預設模型。
TESTS:
- `pnpm test`（設定了 `BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw`）→ 結束碼 0。
  - Rust：1610 passed、0 failed、8 ignored。
  - 前端：vitest 29 個檔、619 tests 全部通過。
  - 真實瀏覽器測試有實際執行，沒有略過：`browser_driver.rs` 2 passed，`web_verification_real_browser.rs` 1 passed。
- `pnpm build` → 結束碼 0。release 的 `baton-runtime`、`baton-guardian` 與 `baton-desktop.exe` 都建置成功。唯一的警告是 `baton-api/src/shared_settings.rs:20` 未使用的 import，該檔不在本票的 diff 內。
FIXES_NEEDED: none

不阻擋通過、但修正或整合時可以留意的事：
1. 跟 v1（票 49）合併時，migration 43 與 AI-425～AI-430 的編號會撞到，必須順延。
2. `act` 的參數如果含 `{{run_id}}` 之類的佔位字：錄製身分用的是原文，錄下的具體動作卻是代換後的值。重播會帶入舊的值，接著因後置條件不成立而變成 stale。這不會沿用舊成功，但這類步驟實際上無法重播。
3. 同一個 Workspace 的兩個驗證同時跑時，`apply_replay_updates` 是無條件取代或標記 stale，後寫的會蓋過先寫的。結果偏保守，影響低。
