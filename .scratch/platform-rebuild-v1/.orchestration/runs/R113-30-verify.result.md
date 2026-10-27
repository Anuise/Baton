```
VERDICT: PASS
CRITERIA:
- [PASS] Controller 換帳號後對話連續、待處理訊息不遺失也不重複 — 測試 `crates/baton-harness/tests/controller_handoff.rs` 共 4 個情境覆蓋這條（Runtime harness，走真實 HTTP API）：
  (1) `the_next_turn_moves_to_another_account_…`：帳號 A 達 0.9 進入 draining 後，下一輪改在帳號 B 執行。測試斷言 conversation_id 不變、`handoff={from_run_id, from_profile_id:A, reason:quota_draining}`、不帶 `--resume`、新 Session 的 `replacement_reason=account_changed` 且 predecessor 指向舊 Session。Prompt 的背景含已處理對話、決策（「比較表要用哪種格式？ → Markdown 表格」）與檢查點；「本輪要處理」區段只有第二則指示。兩則 turn 都是 processed、attempts=1。
  (2) `a_turn_running_when_its_account_drains_…`：等滿 2 分鐘（CHECKPOINT_WAIT_MS）之前不會呼叫 stop。程序回 StillAlive 時不建立第二輪（B1）；ForceKilled 之後舊輪以 `handed_off…` 結束，新輪的 `input_turn_ids=[u1,u2]` 依到達順序，每則在 Prompt 中只出現一次。只有一則回覆、使用者訊息沒有重複。
  (3) `a_sudden_limit_…`：收到 Exhausted 後同一次 tick 就在帳號 B 接手，沒有退避。
  (4) `a_handoff_survives_a_runtime_restart…`：停止途中崩潰、重建後仍以交接結束，承接的那一輪只啟動一次。
  實作位置：`crates/baton-domain/src/conversation.rs:551`（交接後不退避）、`:814`（`handed_off_if`）、`:289`（`handoff_stop_due`）；`crates/baton-runtime/src/supervisor.rs:1398`（`stop_controllers_for_handoff`）、`:1502`（換帳號時記為交接）；`crates/baton-store/src/conversations.rs:651`（`load_background`）。
- [PASS] Controller 無可用帳號時事件被保存，UI 顯示等待原因，Runtime 其餘功能正常 — 由 `without_an_available_account_…` 覆蓋。唯一帳號達 0.8 時：使用者訊息維持 pending，不喚起 Controller；`view.waiting.code=quota_near_limit`，訊息含「帳號 A」，持續等待時 `since` 保持不變。這段期間 Worker 仍能回報進度、正常結束，Task 前進，成果保存為 pending 的工作事件。帳號恢復後依序處理使用者訊息與工作事件各一次，等待原因清除。
  UI 部分：`web/src/ConversationPage.tsx` 新增「Controller 等待可用帳號：…（訊息已保存）」與交接標示，`ConversationPage.test.tsx` 新增 79 行測試。存檔與清除分別在 `conversation.rs:306`（`wait_for_account`）和 `begin_turn` 中清除 waiting。
- [PASS] 以 Runtime harness（D-04）寫的情境測試涵蓋本票行為並通過；AI 決策已追加 — 5/5 通過。`ai-decisions.md` 追加了 AI-327（高）、AI-328～330（中）、AI-331（低）。
  實作者沒有先看到紅燈，所以我做了突變驗證：暫時改壞實作、只跑 controller_handoff 測試，每次都以 `git checkout` 還原並確認工作區乾淨。
  · M1 交接後仍套用退避 → 2 個測試失敗
  · M2 `handed_off_if` 不生效 → 3 個失敗
  · M3 不呼叫 `stop_controllers_for_handoff` → 2 個失敗
  · M4 Prompt 不附決策 → 1 個失敗
  · M5 不記錄等待原因 → 1 個失敗
  · M6 不等安全邊界就停止 → 2 個失敗
  · M7 沒有承接帳號仍停止進行中的一輪 → 沒被抓到：baton-harness 加 baton-runtime 全部 576 個測試都通過。
  M7 對應的是 AI-327 的細部決策「沒有承接帳號就留在原帳號」。就算這段壞掉，輸入仍會退回待處理、UI 仍會顯示等待原因，所以驗收條件本身仍成立。我判定為覆蓋缺口，不構成 FAIL。
TESTS:
- `pnpm test`（cargo test --workspace --exclude baton-desktop，再加 vitest）→ 結束碼 0。Rust 1348 passed / 0 failed（含 controller_handoff 5/5、conversation 15/15）；前端 20 個檔案、466 個測試全部通過。
- `pnpm build` → 結束碼 0。release 建置成功，產出 baton-desktop.exe。有 1 個警告：`crates/baton-api/src/shared_settings.rs:20` 未使用的 `StoreError` import。這個檔案不在本票 diff 中，是既有問題。
- 突變測試 M1～M7 的結果見上。結束後 `git status --porcelain` 為空，沒有留下任何修改。
FIXES_NEEDED: none
```

需要另外注意三件事（都沒有計入判定）：

1. **前置條件在驗證途中失效。** 驗證開始時 `rebuild/v1`（51787a3）是 HEAD 的祖先；驗證途中整合分支前進到 c8cef95（併入了票 17），現在已不是祖先。票 17 已經用掉第 33 個 migration（`crates/baton-store/src/lib.rs:1213`）和 AI-327～AI-334。本分支同樣用了 migration 33 與 AI-327～331，合併時一定會衝突。conversation 與 supervisor 也很可能衝突（票 17 改過 Controller）。整合 run 需要把本票的 migration 順延為第 34 個、AI 編號改為 AI-335 起，並在整合後重跑全部測試。
2. **補一個情境測試（建議，不阻擋）。** 情境是唯一帳號進入 draining、而 Controller 有一輪正在執行；預期等滿 2 分鐘後不停止，那一輪留在原帳號，對應 M7。
3. **小缺陷（建議，不阻擋）。** `stop_controllers_for_handoff`（`supervisor.rs:425`）找承接帳號時傳入 `active_runs: &[]`，沒有考慮各帳號目前的並行量。承接帳號滿載時，可能停掉舊輪之後下一輪才發現放不下，只能改成等待。
