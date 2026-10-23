VERDICT: FAIL

本票的 5 條驗收條件，對照實作者當時合併的 `cli/v1`（3df1318）全部通過，建置與測試也都通過。判 FAIL 的原因只有一個：驗證進行中，`cli/v1` 合併了票 07（reflog：17:04:59 合併 fb08d28，17:11:03 推進到 6c36938）。現在 `cli/v1` 已不是 HEAD 的祖先，兩者合併會衝突。其中一處衝突處理不好，會讓票 07 加上的憑證失效檢查消失，所以要先整合再重驗。

CRITERIA:
- [FAIL] `cli/v1` 是 HEAD 的祖先 — 剛開始驗證時 `git merge-base --is-ancestor cli/v1 HEAD` 成功；現在重跑結束碼是 1，`git log HEAD..cli/v1` 多出票 07 的 8 個 commit。用 `git merge-tree --write-tree cli/v1 HEAD` 模擬合併（不動工作樹）的結果：
  - `crates/baton-runtime/src/supervisor.rs` 有內容衝突（完全退出流程，以及票 07 改到的 `drive_controllers` 區塊）。
  - `crates/baton-store/src/conversations.rs` 是 modify/delete 衝突：票 07 在 `controller_run_active` 加了 `rebound_at` 檢查，也就是 Runtime 重啟後舊憑證要失效；本票刪掉這個檔案，並在 `crates/baton-store/src/controller_cli.rs:37-50` 重寫了一個沒有這個檢查的版本。
  - `.scratch/desktop-controller-cli/ai-decisions.md` 雖然自動合併成功，但編號重複：票 07 已用了 AI-23～AI-26，本票又用了 AI-23～AI-25（合併後第 228～255 行與第 262～280 行）。
- [PASS] UI 不再有「對話」頁簽與相關元件 — `web/src/App.tsx` 的頁簽型別、`WORKSPACE_TABS` 與 switch 都拿掉了 `conversation`。`ConversationPage.tsx` 和它的測試已刪除，`runtimeClient.ts`、`fixtures.ts`、`styles.css` 的相關內容也已移除，Tauri 指令與 `baton-client` 的方法同樣刪除。在 `web/src`、`src-tauri` 裡 grep `ConversationPage|getConversation|conversation` 沒有殘留。另外 `App.test.tsx:429` 新增了一個測試：本機記住的頁簽是 `conversation` 時，改開 Controller 頁簽。
- [PASS] 使用者訊息或工作事件不再產生任何自動 Controller Run（以 harness 驗證）— `supervisor.rs` 刪掉了 `drive_controllers`、喚醒與交接，約 450 行。`crates/baton-harness/tests/retired_conversation.rs` 有兩個測試：
  - `the_conversation_endpoints_are_gone`：兩個對話端點都回 404；經過一小時的 fake 時間後，沒有任何 `ctl_` spawn，也沒有開出 Controller 終端機。
  - `worker_results_blockers_and_answered_controller_questions_never_start_a_controller_turn`：Worker 成功結束、Worker 回報受阻、Controller 提出的裁定被回答，三種事件之後都是 0 次 `ctl_` spawn，終端機只有使用者自己開的那一個。
  - 兩個測試實際執行都 ok。
- [PASS] 既有對話資料仍在資料庫，升級後不遷移、不刪除 — 本票沒有新增 migration，`lib.rs` 只拿掉 `mod conversations`。Store 的非測試程式碼裡也已沒有寫入 `conversation`、`conversation_turn` 或 `kind='turn'` 列的語句（grep 確認）。`legacy_conversation_data_stays_untouched_across_restarts` 先直接寫入升級前的資料，重啟前後各經過一小時，資料都原樣保留，測試 ok。
- [PASS] 被移除功能的測試一併移除，其餘測試通過；GLOSSARY 與 ADR-0009 一併更新 — 已刪除 `conversation.rs`、`controller_handoff*.rs`、`conversation_client.rs`、Domain 與 Store 的 conversation 測試和 `ConversationPage.test.tsx`。其餘 harness 測試改用 `Harness::open_controller_cli`（`crates/baton-harness/src/lib.rs:313`）取得憑證。GLOSSARY 新增「conversation（已退役，票 08）」詞條，Workspace、Run、Session、Controller 等詞條也一併調整。ADR-0009 的狀態改為「部分被 ADR-0014 取代（A1、B1）」。測試結果見下方 TESTS。
- [PASS] AI 決策已追加至 ai-decisions.md — AI-23～AI-25 已加入並標了重要度（中、中、低）。但和 `cli/v1` 合併後編號會重複，見第一條。

TESTS:
- `node scripts/test.mjs`（全部測試）結束碼 1：Clippy 通過（`-D warnings`），前端 30 個檔案共 656 個測試通過，UI E2E 3 個通過。Rust 有 1 個測試失敗：`baton-store lifecycle::tests::concurrent_writers_of_the_lifecycle_file_do_not_trip_over_each_other`，375 次寫入失敗，錯誤是「存取被拒。(os error 5)」。本票沒有改到這個檔案，看起來是 Windows 在高負載下偶發的問題。
- 因為 cargo 碰到第一個失敗就停止，我用 `scripts/with-msvc.ps1 cargo test --workspace --exclude baton-desktop --no-fail-fast` 重跑全部 Rust 測試：結束碼 0，130 個測試目標都是 ok，包含上面那個 lifecycle 測試和 `retired_conversation` 的 3 個測試。
- `node scripts/build.mjs`（建置）結束碼 0：release 建置成功，產生 `target/release/baton-desktop.exe`。
- 以上都是對照舊基準 3df1318 的結果；本分支和合併了票 07 的 `cli/v1` 組合起來還沒測過。

FIXES_NEEDED:
1. 把目前的 `cli/v1`（6c36938，含票 07）合併進 `cli/08-retire-conversation`。
2. `crates/baton-store/src/conversations.rs`：維持刪除，但把票 07 的 `rebound_at` 檢查移到 `crates/baton-store/src/controller_cli.rs` 的 `controller_run_active`。查詢要同時帶 `kind = 'cli'` 和 `rebound_at IS NULL`，也就是 `status` 是 `starting` 或 `running`，而且 `rebound_at` 為空，憑證才有效。要確認票 07「重啟後重新綁定時舊憑證失效」的 harness 測試仍然通過。
3. `crates/baton-runtime/src/supervisor.rs`：
   - 完全退出流程要保留票 07 的 Controller CLI 停止部分：`open_controller_cli_runs`、`pending_controller_cli`、`stop_controller_clis_for_exit`，以及和 Worker 一起 `join`。
   - 移除舊 Controller 輪次的停止：`stop_controllers_for_exit` 和 `controllers` 的分支。
   - 票 07 在 `drive_controllers`、喚醒、交接區塊改的內容一併刪除。
   - 拿掉因此變成沒用到的 import。
4. 本票在 `ai-decisions.md` 的條目改編為 AI-27～AI-29，不要動既有條目。同時檢查 commit 訊息、票面或其他文件有沒有引用本票的 AI-23～AI-25，有的話一起更新。
5. 確認票 07 新增或修改的 harness 測試（Controller CLI 的退出與恢復）沒有依賴 conversation 或對話端點。有的話，比照 AI-25 的做法改成用 Controller CLI 取得憑證。
6. 重跑 `node scripts/test.mjs` 和 `node scripts/build.mjs`，兩者都要通過。如果 lifecycle 並行寫入的測試又失敗，單獨重跑確認只是偶發，並把結果寫在回報裡。
exit: 0


---

主控補充（2026-10-23，派修正 run 前）：

- 驗證之後 `cli/v1` 又合併了票 06（事件等待，63e2785）。現在 `git merge-tree cli/v1 cli/08-retire-conversation` 的衝突為：`crates/baton-api/src/lib.rs`、`crates/baton-domain/src/lib.rs`、`crates/baton-runtime/src/supervisor.rs`、`crates/baton-store/src/lib.rs`，以及 `crates/baton-store/src/conversations.rs`（本票刪除、票 07 與票 06 都修改過）。
- FIXES_NEEDED 第 2 項擴大：`conversations.rs` 裡票 07（`rebound_at`）與票 06（事件等待用到的 Controller 憑證範圍、收件匣事件等）所有仍屬 Controller CLI 的邏輯都要搬到 `controller_cli.rs` 或其他保留的模組，不能遺失；票 06 的 `controller_wait` 情境測試與 shim `wait` 測試都要通過。
- FIXES_NEEDED 第 4 項的編號改為：`cli/v1` 已用到 AI-32，本票條目順延為 AI-33～AI-35。
- 第 5 項同樣適用於票 06 新增的 harness 測試（`controller_wait.rs`）。
