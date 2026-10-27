SCOPE: crates/baton-domain、crates/baton-store — Domain 與 Store（互動式 Controller Run 狀態機、換帳號、恢復與重新綁定、Profile 目錄來源、事件游標、Scheduler 帳號排除、migration 與資料一致性、對話退役後的殘留）

FINDINGS:
- [低] 已要求換帳號時，新 Run 可能不會開出來，而且沒有任何提示。`end_cli` 只在呼叫端傳入 `next` 時才開新 Run；Supervisor 是依這一輪開始時讀到的舊快照判斷 `switch_to_profile_id`，才決定要不要傳 `next`。會出事的時序是：同一輪 Supervisor 迴圈中，前面的 Run 正在停止宿主（寬限 2 秒）或正在查 Session，這時使用者按「換帳號重開」；接著這個 Run 走到宿主消失（`host_gone`）或開窗失敗。Store 讀到的 Run 已經帶有 `switch_to_profile_id`，但 `next = None`，所以只結束舊 Run、不開新 Run。API 的 `host_ended` 一律傳 `Some(next)`，兩邊的作法也不一致 — `crates/baton-store/src/controller_cli.rs:264`（呼叫端見 `crates/baton-runtime/src/supervisor/controller_cli.rs:280-289`）— 建議改成由 Store 在交易內依讀到的最新 Run 決定要不要開新 Run，呼叫端一律提供 `OpenIds`（Supervisor 跟 API 一樣總是產生 ids）。
- [低] `begin_launch` 沒有檢查 `end_requested_at`，使用者已按「結束」或「換帳號重開」後，仍可能用舊帳號開出視窗。Supervisor 只檢查迴圈開始時快照裡的 `end_requested_at`；Store 雖然在交易內重新讀了 Run，但 Domain 的 `begin_launch` 照樣寫下 launch intent。結果是開窗、下一輪又被終止；換帳號時會先用使用者剛換掉的帳號（可能正是額度不足的那個）開一次 `claude` — `crates/baton-domain/src/controller_cli.rs:357-377` — 建議 `begin_launch` 遇到 `end_requested_at` 有值時，直接以 `user_ended`／`switched` 結束 Run，不寫 launch intent，並補一個 Domain 單元測試。
- [低] 從備份還原時，只中和 Worker Run，沒有處理 Controller CLI Run，跨票整合後留下缺口。`end_unfinished_runs` 只處理 `load_active_runs`。如果備份剛好在 Controller CLI 處於 `starting`、還沒寫 launch intent 的時候建立，還原並啟動後，Supervisor 的 `launch_controller_cli` 會自動開出一個 Controller 終端機，使用者並沒有要求。其他非終態的 CLI 列則要等 AI-24 的查驗才會收尾 — `crates/baton-store/src/backup.rs:413-437` — 建議在同一個交易中把非終態的 `kind='cli'` Run 以「還原」為原因結束或中斷；這個操作屬於還原、不算遷移，不違反 AI-38。
- [低] 「憑證是否有效」的規則現在有三份，而且彼此不一致：
  - Domain 的 `credential_valid` 只有測試在用，正式程式碼沒有呼叫。
  - Store 用 SQL 另外實作了一份 `controller_run_active`（`kind='cli' AND rebound_at IS NULL`）。
  - `messages.rs` 判斷 Controller 寄件者 `is_current` 時，只看 status，沒有篩 `kind`、也沒看 `rebound_at`。

  目前 API 已經擋住所有 Controller 呼叫，所以不構成漏洞，但之後改其中一份規則時很容易漏改另一份 — `crates/baton-domain/src/controller_cli.rs:462`、`crates/baton-store/src/controller_cli.rs:44`、`crates/baton-store/src/messages.rs:92` — 建議 `messages.rs` 也加上 `kind='cli' AND rebound_at IS NULL`，並刪除沒有讀者的 `credential_valid`，或改由 Store 呼叫它。
- [低] 「結束中」時重新開啟，顯示的錯誤原因不對。`open` 遇到已要求結束、但還沒結束的 Run，回的是 `AlreadyOpen`（「請切到已開啟的視窗，或先結束它」），可是使用者已經按過結束了。專為這種情況寫的 `Ending`（「正在結束，請等它結束後再開啟」）只有 `request_switch` 用到，那句「再開啟」反而對不上換帳號的情境 — `crates/baton-domain/src/controller_cli.rs:224-226`、`:257` — 建議 `open` 遇到 `end_requested_at` 有值的 Run 時回 `Ending`，`request_switch` 的訊息改成配合換帳號的說法。
- [低] 新增的常數 `AI_DECISIONS_RELATIVE_PATH` 除了定義之外，全 repo 沒有任何讀者；Supervisor 用的是 `ai_decisions_path()` — `crates/baton-domain/src/playbook.rs:20` — 建議刪除，或讓操作手冊的測試用它來核對手冊裡寫的路徑。

已知待修清單（review-notes 第 7、9、10 項：`notices`／`ConversationNotice`／`SessionOwner::Conversation`／`WorkflowNote` 的殘留、`controller_cli_accounts` 沒有呼叫端、中斷時遺失換帳號要求）都已確認仍然存在，上面不重複列出。

SUMMARY: Domain 與 Store 這部分整體結構清楚：規則集中在 Domain，Store 在單一交易內「載入 → 判定 → 寫入」，註解都是繁體中文。`controller_cli_run` migration 的重建保留了舊輪次的資料、觸發器與外鍵，每個 Workspace 只能有一個開啟中 CLI 的限制同時由 Domain 和唯一索引把關。Scheduler 的帳號排除涵蓋了一般派工、佇列重試與交接承接三條路徑；事件游標只會往前、跨 Run 接續的邏輯也符合 AI-27～29。我跑了 `cargo test -p baton-domain -p baton-store`，全部通過（domain 773、store 125）。這次沒有找到高或中嚴重度的問題；剩下的主要風險是 Supervisor 用舊快照帶入決策參數造成的時序缺口，以及還原流程還不認得 Controller CLI Run，都屬於低機率情境，建議在最終修正時一併處理。
exit: 0

