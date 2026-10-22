VERDICT: PASS
CRITERIA:
- [PASS] 前置：`rebuild/v1` 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 成功（ANCESTOR_OK），rebuild/v1 的最新 commit 是 e35a294。
- [PASS] 隔天開啟裁定中心，請求完整；目標版本已變更的請求不能套用 — 證據如下：
  - 隔天開啟：`crates/baton-harness/tests/decision_center_full.rs:388` 先把 fake clock 往後推一天，再 `harness.restart()`。之後逐欄確認 question、reason、options、recommendation、evidence、urgency、blocking、target、allowed_actions 都完整，總覽數字正確，請求仍能接受並恢復 Task。
  - 目標版本變更：`:444`、`:479` 修改契約完成條件後，請求在開啟列表或提交時被標為 `superseded`，`allowed_actions` 變成空陣列；之後套用回 409 `decision_superseded`，`decision` 表沒有寫入任何資料（0 列），只被它阻擋的 Task 回到 ready。
  - `expected_revision`：`:523` 版本過期回 409 `decision_revision_mismatch`，缺少這個欄位回 400，兩種情況都沒有寫入。
  - Store 實作：`crates/baton-store/src/decisions.rs:161-186` 在提交時標記過期；`:232-249` 開啟列表時先 Refresh；`:266` 狀態列計數前也先 Refresh。
- [PASS] 批次裁定每項各自保存，不形成概括授權 — 證據如下：
  - `decision_center_full.rs:715`：三項批次中，第 2 項版本不符而失敗，第 1、3 項仍各自寫入，各帶自己的 action、option、note 和共同的 `batch_id`；失敗那一項保持原狀（revision 1）。空批次和重複項目回 400 `decision_batch_invalid`，什麼都不寫。
  - `:776`：批次核准兩個 `git push` 權限提示，每個只能用一次；同一個 Task 換一個不同的操作（`--force`）或第二次執行，都要重新裁定。
- [PASS] Controller 試圖修改完成條件或跳過 Gate 被拒絕並轉為裁定請求 — 證據如下：
  - 權限表：`crates/baton-domain/src/authority.rs` 內建 spec D-15 的 17 個事項，`ModifyDoneCriteria` 和 `SkipStageOrGate` 都是 `RequiresDecision`。`replace_policy` 只允許收窄；放寬或修改只能人工發起的事項，回 `authority_widening_refused`。
  - API：`crates/baton-api/src/agent.rs` 的 `controller_action`。
  - 修改完成條件：`decision_center_full.rs` 第 948 行起的測試 `a_controller_trying_to_modify_done_criteria_is_refused_and_turned_into_a_request`。回應 `requires_decision`，建立 kind=authority 的請求，契約版本不變，Task 進入 waiting_decision，另一個 Task 不受影響。核准只能用一次，第三次呼叫又建立新的請求。
  - 跳過 Gate：`a_controller_trying_to_skip_a_gate_is_refused_...`。駁回後回 `denied`，Task 沒有完成。
  - 收窄與放寬：`autonomous_matters_are_allowed_until_the_workspace_narrows_them` 涵蓋收窄、放寬被拒和 revision 不符。
  - 沒有繞過途徑：`crates/baton-api/src/guard.rs:153` 規定 Agent 憑證只能用 `/agent/v1`。修改契約的端點只接受桌面憑證，所以 Controller 無法自行改契約。
- [PASS] 以主要 seam（Runtime harness，D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加 — 證據如下：
  - `crates/baton-harness/tests/decision_center_full.rs`：23 passed, 0 failed。測試透過真實 HTTP API、fake Port，並包含 restart 情境。
  - `.scratch/platform-rebuild-v1/ai-decisions.md`：新增 AI-208～AI-216，只有新增行（79 行），沒有刪改既有條目。
- 補充（spec 的其他要求）：
  - D-21 狀態列待裁定數：`web/src/StatusBar.tsx:77`；情境測試 `the_status_bar_counts_only_requests_that_still_need_a_decision` 確認已回答、被合併、已過期的請求不計入，延後的仍計入。
  - 先總覽再逐項：`web/src/DecisionCenter.tsx:268`。
  - migration 20：我另外用 Python 在記憶體中的 sqlite 3.49.1 上，對含已回答、待處理和權限提示的舊資料執行了這段 SQL。資料完整搬移，`foreign_key_check` 為空，外鍵參照在改名後正確更新。

TESTS:
- `pnpm test`（`cargo test --workspace --exclude baton-desktop` 加 `pnpm --filter web test`）：結束碼 0。
  - Rust：全部通過，0 failed（有 5 項和 3 項 ignored，都是既有測試）。
  - 前端：18 個測試檔、352 項全部通過。
- `pnpm build`：結束碼 0，release 建置完成，產出 `target/release/baton-desktop.exe`。
- 驗證後 `git status --short` 為空，沒有改動任何追蹤中的檔案。

FIXES_NEEDED: none

有一個不影響驗收的小瑕疵，可以之後順手修：`crates/baton-api/src/agent.rs:436` 的 `requires_decision` 回應訊息中間夾了一段約 17 個空白（「已建立裁定請求。」與「請等待裁定結果」之間）。這是字串內真的有的空白，不是排版造成的；可以改用 `\` 續行，或刪掉這些空白。
