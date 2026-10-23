SCOPE: web/、src-tauri/、scripts/、docs/、GLOSSARY.md、crates/baton-harness — Web UI、Tauri 殼、腳本、文件與 harness 測試（頁簽外殼、Controller 頁簽、帳號畫面、測試是否真的涵蓋各票驗收、不穩定測試、文件與實作一致）
FINDINGS:
- [中] review-notes 第 2、5 項不穩定測試的成因應該就在這裡：這些「模擬在某一點當掉」的測試，靠真實時間的 `tokio::time::timeout(500ms, supervise())` 丟棄 tick，並假設 500ms 內一定會走到停住點。高負載下若 supervise 還沒走到 hold 或 hang 點就被丟棄，當掉的位置就會提早，後面的斷言（例如 `"removing"`、`"merging"`、sorting 旅程的 verdict）就會失敗。review-notes 點名的三支測試都是這種寫法 — `crates/baton-harness/tests/verification_handoff.rs:226`、`crates/baton-harness/tests/worktree_cleanup.rs:352`、`crates/baton-harness/tests/worktrees.rs:430`。同樣寫法還有 `handoff.rs:222`、`runs.rs:162`、`sessions.rs:227`、`outbox_and_external_ops.rs:123`、`retry_budgets.rs:331`、`acceptance.rs:315`、`run_process_release.rs:187`、`artifacts.rs:310`、`verification_takeover_blocked.rs:84` — 建議修法：改照本 feature 已有的寫法 `controller_cli.rs:271-278`（`select!` supervise 與 `wait_for_terminal_hang()`），或 `worktrees.rs:270` 的 `wait_for_held_merge()`、`lifecycle.rs:453` 的 `wait_for_spawn_hang()`。做法是在 fake 的 `hang_next_http`、`hold_next_removal`、`hold_next_merge` 等停住點加上「已走到」的通知，測試改成等通知，不再用 500ms 估計；不要放寬斷言。
- [中] spec 使用者故事 41 與票 03 驗收「時間線記錄開啟、結束與帳號」在 UI 上沒有做到。時間線頁每筆只顯示時間、類別與原始 `event_type`。Controller CLI 事件的 `run_id` 是 null、帳號只在 `payload.profile_id`，所以使用者在畫面上看不出是哪個帳號、哪一次開啟。harness 只在 API 的 payload 驗證了帳號，前端測試 `TimelinePage.test.tsx:178-194` 也只斷言類別與 `event_type` — `web/src/TimelinePage.tsx:167-172` — 建議修法：`controller` 類別的事件顯示 `payload.profile_id` 對應的帳號名稱（以及 `aggregate_id`／`end_code`），並補元件測試斷言帳號名稱出現在時間線項目中。
- [低] 補充 review-notes 第 8 項：ControllerPage 的輪詢測試用 `useFakeTimers({ shouldAdvanceTime: true })`，只留「一秒餘裕」。從計時器註冊到 `toHaveBeenCalledTimes(1)` 之間，只要真實時間超過 1 秒，interval 就會提早觸發，斷言跟著失敗，這正是高負載下會遇到的情況。`AccountsPage.test.tsx:711-731` 是同一種寫法 — `web/src/ControllerPage.test.tsx:331-345` — 建議修法：改用同檔 `:385-404` 已採用的「很短的 pollMs＋真實計時器＋以呼叫次數下限斷言」，或關掉 `shouldAdvanceTime` 並改用 `advanceTimersByTimeAsync`。
- [低] Controller CLI 開著時若帳號清單讀取失敗，畫面不顯示任何錯誤，「換帳號重開」區塊就這樣消失。`profilesError` 只在未開啟的分支顯示 — `web/src/ControllerPage.tsx:255`（對照 `:281`）— 建議修法：把「無法取得帳號清單」的提示移到兩個分支共用的位置，並補一個「開著時帳號清單讀取失敗」的元件測試。
- [低] GLOSSARY 有幾處與實作不一致：
  - MCP shim 仍定義為「Worker 程序用來…、隨 Worker Run 啟動」，沒有提到 Controller CLI、Controller 宿主子命令（會啟動 `claude` 並回報生命週期）與 `wait` 子命令（`GLOSSARY.md:144-147`）。
  - 等待原因的「帳號」類沒有列入 `controller_account`（`GLOSSARY.md:258`）。
  - Agent 憑證寫「有效與否取決於該 Run 是否仍是 Task 的寫入所有者」，但 Controller 憑證是在 Controller Run 結束或重新綁定（`rebound_at`）時失效（`GLOSSARY.md:141`）。
  - Account Profile 沒有提到外部目錄來源（`config_dir_origin`，以及封存時不刪目錄）（`GLOSSARY.md:80-83`）。

  建議修法：更新上述四個條目。
- [低] harness 的 `Harness::open_controller_cli` 從「所有」開窗紀錄中取最後一個帶憑證的，沒有確認這次開啟真的開窗並進入 running。若這次開啟失敗（例如 `claude_not_found`），它會默默回傳上一次開窗的舊憑證，測試會以難以理解的 401 失敗 — `crates/baton-harness/src/lib.rs:316-331` — 建議修法：supervise 之後讀 `GET controller-cli` 斷言 `status == "running"`，並以該 Run 的 `command_id` 或 `working_dir` 比對開窗紀錄（同 `controller_cli.rs:73-86` 的 `open_and_launch`）。
SUMMARY: 這個範圍的實作整體與 spec、AI 決策一致：頁簽外殼、Controller 頁簽、以既有目錄建立 Profile、Tauri 指令與 HTTP／Tauri client 都對得上；harness 情境測試也逐條對應票 03、05、06、07、08 的驗收；Windows 驗收清單裡的訊息文字我都在程式中找到了。最值得先修的是第一項：review-notes 那幾支不穩定的 harness 測試，成因很可能是共用的真實時間 500ms 當掉模擬，本 feature 已有不依賴時間的寫法可以照搬。其次是時間線畫面看不到 Controller 帳號，故事 41 只在 API 層達成。這次審查只讀了程式與 diff，沒有執行任何測試。
exit: 0

