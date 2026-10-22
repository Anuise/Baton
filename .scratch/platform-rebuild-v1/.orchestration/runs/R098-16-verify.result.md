VERDICT: FAIL

**前置條件：** `git merge-base --is-ancestor rebuild/v1 HEAD` 回傳 `ANCESTOR_OK`，`rebuild/v1` 的頂端 `b0d46a1` 是 HEAD 的祖先。

CRITERIA:
- [PASS] Controller 忙碌時收到兩則使用者訊息與一個工作事件，處理順序正確且不並行 — 證據是 `crates/baton-harness/tests/conversation.rs:204-290`。忙碌時 u2、u3 和工作事件都是 `pending`（第 230-233 行）；等了 3 分鐘，進行中的輪次仍只有 1 個，spawn 也只有 1 次（第 237-242 行）。第二輪的 `input_turn_ids` 是 `[u2,u3]`（第 255 行），依到達順序排列，而且不含工作事件；第三輪才處理工作事件（第 269 行），總共 spawn 3 次。不並行另由兩處保證：唯一索引 `controller_run_one_active`（`crates/baton-store/src/lib.rs` 的 migration 28），以及 `plan_wake` 遇到 Busy 就不喚起（`crates/baton-domain/src/conversation.rs:461-480`）。測試結果是 ok。
- [PASS] 無事件時不產生任何 Controller Run — 證據是 `conversation.rs:165-201`。過 5 小時仍是 0 次 spawn；還沒有對話時 Worker 結束了，也不喚起；處理完一輪後再過 5 小時，仍只有 1 次 spawn。`controller_wakes` 只查有 `pending` turn 的對話（`crates/baton-store/src/conversations.rs:102-137`）。測試結果是 ok。
- [PASS] Controller 換 Session 後 conversation_id 與未處理指示保留且不重複執行 — 證據是 `conversation.rs:292-367`。換成 NotResumable 後建立了替代 Session：`conversation_id` 不變；`predecessor_session_id` 和 `history_gap` 都有值；替代 Session 不帶 `--resume`。已處理的指示只出現在背景區段，本輪區段只有第三則。每則 `attempts == 1`，共 spawn 3 次。測試結果是 ok。
- [FAIL] 在喚起與結果保存邊界中斷後，未完成事件可找回 — 三個 crash 測試都通過（`conversation.rs:369-469`），但有一個缺陷讓這條在實際環境不成立：
  - **缺陷：** 執行中的 Controller Run 沒有任何存活核對。`pump_controller` 只在事件串流結束時才查證程序，條件是 `ended && controller_process_is_dead`（`crates/baton-runtime/src/supervisor.rs:1140-1146`）。但真實 `ClaudeCodeAdapter::events` 讀到 `Exited` 之前不會結束串流，而是一直追蹤檔案（`crates/baton-runtime/src/claude_adapter.rs:160-203`）。Worker Run 有 lease 過期後的 `check_leases` 可以查證，Controller Run 沒有對應的機制。
  - **後果：** Controller 程序沒寫出 result 就死掉時（崩潰、被殺、整機重開、Runtime 一起當掉），這一輪會永遠停在 `running`，輸入一直是 `processing`，B1 也讓之後的輪次全部被擋住。
  - **重現：** 我寫了暫存測試，跑完已刪除，worktree 仍是乾淨的。fake 程序設為 `Liveness::Dead`、不送 `Exited`，然後推進 10 小時。重啟與不重啟兩種情況都輸出 `RUN_STATUS="running" TURN_STATUS="processing" BUSY=true`。
  - **既有測試為什麼會過：** `a_crash_before_the_result_is_saved_...` 能通過，是因為 fake 的通道已經消耗過 `Exited`（`channel.ended = true`），重新接上時串流立刻結束。這和真實 Adapter 的契約不一致：真實 Adapter 會從頭重播，否則就不會結束。所以 AI-285 寫的「重建後沒有結束事件時查證程序」，在真實 Adapter 上不會觸發。
- [PASS] 以主要 seam（Runtime harness）撰寫的情境測試涵蓋本票行為並通過；本票的 AI 決策已追加至 ai-decisions.md — `crates/baton-harness/tests/conversation.rs` 有 10 個情境測試，全部通過，使用真實 HTTP API、fake Port 和 fake Clock。AI-280～AI-291 已追加，`ai-decisions.md:2523-2630`。
  - 小瑕疵：AI-280 內文仍寫「migration 27」，實際是第 28 個（AI-291 也這麼說）。
  - 補充一點：fake 的事件串流行為和真實 Adapter 不同，這正是上一條缺陷沒被測出的原因。

其他核對：
- UI 有「已收到／待處理」（`web/src/ConversationPage.tsx:25`）。
- 取消在 Controller 忙碌時立即生效（`conversation.rs:499-521`）。
- 完全退出時會停止 Controller 那一輪，輸入退回待處理（`conversation.rs:523-545`）。
- 票 37 留言的 AI-161（`record_lesson_candidate` 是否收窄到 Controller）已在 AI-287 決定維持開放，等票 17。

TESTS:
- `pnpm install --frozen-lockfile`：完成。
- `pnpm build`：exit 0，產出 `target/release/baton-desktop.exe`。有一個警告：`crates/baton-api/src/shared_settings.rs:20` 有未使用的 import `StoreError`，這個檔案不在本票 diff 內。
- `pnpm test`：exit 0。
  - Rust（`cargo test --workspace --exclude baton-desktop`）：76 個測試執行檔，1207 passed、0 failed、8 ignored。
  - 前端 vitest：19 個檔案，426 passed。
  - 其中 `tests/conversation.rs` 10 passed、domain 的 `conversation::tests` 14 passed、`conversation_client` 1 passed。

FIXES_NEEDED:
1. 為執行中的 Controller Run 加上存活核對，不能只依賴事件串流結束。做法可以是：
   - 每次 tick 對 `running` 的 Controller Run 檢查：串流沒有新事件超過某個時間（例如沿用 D-31 的 lease 60 秒），就呼叫 `adopt_controller` 加 `ProcessControl::inspect`。
   - 或者每次 tick 在串流沒有結束事件時直接 inspect。
   
   確認 `Liveness::Dead` 後，再從串流把已到達的事件讀乾淨一次，避免 result 已經寫入但還沒讀到。仍沒有結果就以 `TurnOutcome::Interrupted` 結束，輸入退回待處理。`Unknown` 時不下結論。
2. 新增 harness 情境測試，涵蓋兩種情況，兩者都要斷言這一輪變成 `interrupted`、輸入回到 `pending`，退避後重新交付，而且 Prompt 註明「可能已部分處理」：
   - Controller 程序已啟動並送出 `Started`，之後沒有 `Exited` 就死亡（`script_liveness(Dead)`），Runtime 不重啟。
   - 同樣情況，但 Runtime 重啟（串流仍未結束）。
3. 修正 AI-285 的敘述，讓它和實際機制一致。也可以另外追加一條 AI 決策，說明 Controller Run 的存活核對方式，並標上重要度。
4. （小）AI-280 內文的「migration 27」改為「第 28 個 migration」。依規則不能改既有條目時，在 AI-291 補註即可。
