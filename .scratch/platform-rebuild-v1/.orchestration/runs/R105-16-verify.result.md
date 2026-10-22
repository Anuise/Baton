The fix and full verification are done. HEAD is clean, and I made no changes to any tracked file.

```
VERDICT: PASS
CRITERIA:
- [PASS] Controller 忙碌時收到兩則使用者訊息與一個工作事件，處理順序正確且不並行 — 證據是 crates/baton-harness/tests/conversation.rs:208-296 的 `a_busy_controller_handles_two_user_messages_before_the_work_event_one_turn_at_a_time`，本次執行結果是 ok。測試涵蓋三點：忙碌期間只有 1 個進行中的輪次、spawn 1 次；第二輪的輸入是 [u2,u3]；第三輪才處理工作事件。不並行另有兩層保證：唯一索引 `controller_run_one_active`，以及 `plan_wake` 遇到 Busy 就不喚起。這部分的邏輯自 R098 以來沒有變動。
- [PASS] 無事件時不產生任何 Controller Run — 證據是 conversation.rs:170-206 的 `without_events_no_controller_run_is_ever_started`，結果 ok。分三段推進時間，每段 5 小時，期間都沒有 spawn。
- [PASS] Controller 換 Session 後 conversation_id 與未處理指示保留且不重複執行 — 證據是 conversation.rs:297-372 的 `a_session_replacement_keeps_the_conversation_and_runs_pending_instructions_exactly_once`，結果 ok。conversation_id 不變；predecessor_session_id 與 history_gap 都有值；每則 attempts == 1。
- [PASS] 在喚起與結果保存邊界中斷後，未完成事件可找回 — 前次 FAIL 的缺陷已修正。
  - 程式修正：crates/baton-runtime/src/supervisor.rs:1427-1494 的 `pump_controller` 在兩種情況下會查證程序：串流已結束，或已超過 `LEASE_DURATION_MS`（60 秒）沒有收到事件。查證時用 `controller_process_is_dead`，它會先呼叫 `adopt_controller`，再呼叫 `ProcessControl::inspect`。判定流程如下：
    - 結果是 `Dead` 時，只設定 `process_gone`。下一次 tick 再把串流讀乾淨，仍然沒有 `Exited` 才以 `Interrupted` 結束這一輪。
    - 結果是 `Alive` 或 `Unknown` 時不下結論，計時重新起算。
  - fake Adapter 的行為：crates/baton-harness/src/fakes.rs:391-408 的串流沒有 `Exited` 時會一直停在 Pending，和真實 Adapter 的契約一致。所以新測試確實會走到這條新路徑，不會因為串流提早結束而碰巧通過。
  - 新測試（本次執行皆 ok）：
    - conversation.rs:478-543 涵蓋 Runtime 不重啟與重啟兩種情況。程序送出 Started 和 Message 後沒有 `Exited` 就死亡。在 `Unknown` 狀態下推進 10 小時，這一輪仍是 running。改為 `Dead` 並推進 60 秒後，這一輪變成 interrupted，輸入回到 pending，busy=false。退避後重新交付，Prompt 含「可能已部分處理」，attempts=2，總共 spawn 2 次。
    - conversation.rs:545-567：確認程序不在之後才到達的結果仍然會保存（succeeded），spawn 1 次。這條測試涵蓋了競態。
  - 既有邊界測試仍通過：conversation.rs:374-476 的三個測試，涵蓋 spawn 前、spawn 後找回程序、結果保存前崩潰三種情況。
- [PASS] 以主要 seam（Runtime harness）撰寫的情境測試涵蓋本票行為並通過；本票的 AI 決策已追加至 ai-decisions.md — tests/conversation.rs 共 15 個情境測試，全部 ok。整合後的 AI 決策編號是 AI-299～AI-311（ai-decisions.md:2695-2810），對應關係如下：
  - AI-304：更正了前次指出的錯誤敘述。
  - AI-311（重要度：高）：記錄存活核對的做法。
  - AI-310：記錄整合，migration 改為第 30 個。
  - AI-299：內文已寫「第 30 個 migration」，前次提到的「migration 27」小瑕疵已不存在。
  - docs/development.md 有同步更新。

TESTS:
- 前置條件：`git merge-base --is-ancestor rebuild/v1 HEAD` 輸出 ANCESTOR_OK。rebuild/v1 的頂端是 c1b71f8，已包含在 HEAD 292431e 中。
- `pnpm install --frozen-lockfile`：完成。
- `pnpm build`：exit 0，產出 target/release/baton-desktop.exe。有 1 個警告：crates/baton-api/src/shared_settings.rs 有未使用的 import `StoreError`。這個檔案不在本票 diff 內，前次也已存在。
- `pnpm test`：exit 0。
  - Rust：79 個測試執行檔，1279 passed、0 failed、8 ignored。
  - 前端 vitest：20 個檔案，452 passed。
  - 本票相關：tests/conversation.rs 15 passed；conversation_client 2 passed；domain 的 conversation::tests 14 項全部 ok。
- 跑完後 `git status --short` 是空的。

FIXES_NEEDED: none
```

非阻擋性備註：
- AI-311 寫明的已知取捨：程序死掉後最多約 60 秒加一個 tick 才會被找回；崩潰恢復不會停止 Controller 那一輪。這兩點都已記錄，也合理。
- 票 28 整合後，Controller 喚起會遵守額度門檻。有對應的情境測試 `controller_wakes_skip_an_account_held_by_the_quota_gate`，結果 ok。
