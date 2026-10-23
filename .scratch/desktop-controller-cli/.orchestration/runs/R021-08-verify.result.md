VERDICT: PASS
CRITERIA:
- [PASS] 前置：`cli/v1` 是 HEAD 的祖先 — `git merge-base --is-ancestor cli/v1 HEAD` 成功（ANCESTOR_OK）；HEAD 是 `467b339`，已合併 cli/v1，裡面有票 06、07。工作樹乾淨，`git status --short` 沒有輸出。
- [PASS] UI 不再有「對話」頁簽與相關元件 — `web/src/App.tsx` 的 `WorkspaceTab` 型別、`WORKSPACE_TABS` 與 `case "conversation"` 都已移除，也不再匯入 `ConversationPage`。`web/src/ConversationPage.tsx` 與它的測試已刪除，`runtimeClient.ts` 的對話型別與方法、`fixtures.ts`、`styles.css` 中的對話相關部分也已刪除。`src-tauri/src/main.rs` 移除了 `conversation_view`／`conversation_send_message` 指令，`crates/baton-client/src/lib.rs` 移除了 `conversation`／`send_conversation_message`。`web/src` 只剩一處提到 conversation，就是 `web/src/App.test.tsx:429` 的新測試：本機記住的是舊的 `conversation` 頁簽時，會回到 Controller 頁簽。
- [PASS] 使用者訊息或工作事件不再產生任何自動 Controller Run（以 harness 驗證） — `crates/baton-runtime/src/supervisor.rs` 的事件合併、`claude -p` 輪次與 Controller 自動交接整段刪除（diff 減少 512 行），`baton_domain::conversation` 也整個刪除。Store 裡 `runs.rs`、`decisions.rs`、`understanding.rs`、`workflows.rs` 原本寫入 `enqueue_*` 的路徑都已移除。除了 migration 之外，程式碼裡已經沒有任何寫入 `conversation_turn` 或 `kind='turn'` 的地方（已用 grep 確認）。harness 測試在 `crates/baton-harness/tests/retired_conversation.rs`：
  - `the_conversation_endpoints_are_gone`：對話端點回 404。
  - `worker_results_blockers_and_answered_controller_questions_never_start_a_controller_turn`：Worker 成功、Worker 回報受阻、Controller 提出的裁定被回答之後，再讓時間經過一小時，`ctl_` 開頭的 spawn 數為 0；開窗只有使用者開的那 1 次；舊的 turn 資料沒有被寫入。

  三個測試都通過。
- [PASS] 既有對話資料仍在資料庫，升級後不遷移、不刪除 — 沒有新增 migration。`legacy_conversation_data_stays_untouched_across_restarts` 先寫入升級前的資料（conversation、3 筆 turn，其中 1 筆是 `pending`，再加上 `kind='turn'` 的 controller_run），讓時間經過一小時、重啟、再經過一小時，資料原樣保留。`crates/baton-store/src/controller_cli.rs:38-51` 的 `controller_run_active` 改為只認 `kind='cli'`，所以舊輪次的憑證一律無效（見 AI-33）。
- [PASS] 被移除功能的測試一併移除，其餘測試全數通過，GLOSSARY 與 ADR-0009 已更新 — 已刪除的測試：
  - harness：`conversation.rs`、`controller_handoff.rs`、`controller_handoff_single_account.rs`
  - client：`conversation_client.rs`
  - domain：`conversation/tests.rs`
  - 前端：`ConversationPage.test.tsx`、`runtimeClient.test.ts` 中的對話部分

  其他靠對話取得 Controller 憑證的 harness 測試，改用新的 `Harness::open_controller_cli`／`end_controller_cli`（`crates/baton-harness/src/lib.rs:313-342`）。`GLOSSARY.md` 新增「conversation（已退役，票 08）」詞條，並更新 Workspace、Run、Session、Controller、Controller CLI、Message。`docs/adr/0009-...md:4-5` 標示 A1、B1 部分被 ADR-0014 取代，並已在票 08 移除。`docs/development.md:1078-1088` 也新增了退役說明。
  - 不擋驗收的小問題：`crates/baton-domain/src/decision.rs` 的 `DecisionChange.notices` 與 `ConversationNotice`（第 462、730、1707、1962 行）仍會產生，但 Store 已不再讀取。`session::SessionOwner::Conversation` 與 `workflow::WorkflowNote` 在正式程式碼裡也已沒有使用者。`decision.rs` 第 287、459、1698 行的註解仍寫著「喚醒 Controller」，與現況不符。`decision/tests.rs:441` 等測試仍在斷言這些通知。AI-34 把它們列為刻意暫留，理由是「票 06 可能沿用」；但票 06 已經合併進這個分支，而且沒有用到。建議另開清理票處理，本票不判為 FAIL。
- [PASS] AI 決策已追加至 ai-decisions.md — `.scratch/desktop-controller-cli/ai-decisions.md` 新增 AI-33（中）、AI-34（中）、AI-35（低），每條都有重要度、時間／階段、決策、理由、替代方案、影響，以及「審閱：未審」。
- [PASS] 有沒有違反 spec — spec「移除舊對話」那段要求的事都有做到：對話頁、前端的對話 client 方法與 Runtime 的自動喚醒都已移除；資料表保留、不遷移、不再寫入。Controller 工具與 MCP shim 沒有改動。票 06 的事件等待透過事件表通知「裁定被回答」，並由 `controller_wait.rs:143` 通知「收到訊息」，這兩條都不依賴已移除的 conversation 路徑。專案理解的投影衝突與 Gate 失效不再通知 Controller，但 spec 列出的等待事件本來就不包含這兩種，影響也已在 AI-34 記錄。
TESTS:
- `pnpm test`：結束碼 0。Clippy（`-D warnings`）通過；Rust 有 134 個測試目標，1829 個通過、0 個失敗；前端 Vitest 30 個檔案、664 個測試全部通過；UI E2E 3 個通過。
- `pnpm build`：結束碼 0。Runtime、Guardian、Web UI 與 Tauri 的 release 建置都成功，產物是 `target/release/baton-desktop.exe`。
FIXES_NEEDED: none
exit: 0

