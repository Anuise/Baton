```
VERDICT: PASS
CRITERIA:
- [PASS] 模擬接近五小時與週上限，皆觸發 draining 與交接，成果延續 — 五小時上限：五小時視窗 86% 時帳號 A 進入 draining（原因 drain_threshold）、建立 requested 交接並選 B 承接；Worker 的回報回應帶 stop_requested 與 handoff_requested；保存 checkpoint 後，B 的第 2 世代 successor 啟動，Prompt 帶最新 checkpoint 與 confirm_handoff 指示（crates/baton-harness/tests/handoff.rs:229-291）。週上限：七日視窗 90% 同樣進入 draining，Worker 自行結束後轉移，stop_outcome=exited，successor 的 Prompt 帶週上限前的進度（handoff.rs:294-321）。沒有數字的 warning 也會進入 draining，等滿 2 分鐘後帶最近的 checkpoint 交接（handoff.rs:324-347）。successor 沿用 Task 登記的 working_dir（crates/baton-domain/src/run.rs:225-226），所以工作目錄中的成果會延續。門檻遲滯實作在 crates/baton-domain/src/handoff.rs:128-182。
- [PASS] 舊 writer 未確認停止前，新 Worker 不能進入同一可寫目錄 — 只有 StoppedGracefully、ForceKilled、inspect 回 Dead 或 Worker 自行結束才轉移；StillAlive 或停止失敗時交接停在 stopping（crates/baton-runtime/src/supervisor.rs:555-582）。轉移是單一交易，條件是舊 Run 仍持有 from_generation 的寫入所有權（crates/baton-store/src/handoffs.rs:201-275，條件更新在 :227）。情境測試：StillAlive 三次加 Fail 一次，spawn 數維持 1、舊 Run 仍是 running；回 ForceKilled 後才建立 successor（handoff.rs:351-382）。
- [PASS] 交接中各 phase 中斷後重建，依 phase 恢復且不出現兩個 writer — 涵蓋四個中斷點：requested 時重啟（handoff.rs:387）、stopping 途中崩潰再重啟（:406）、崩潰期間舊程序已死，重啟後以 dead 轉移、不再要求停止（:431）、successor spawn 之後崩潰，重啟後以 command_id 找回、spawn 數維持 2（:453）。每個測試都呼叫 assert_single_writer，同時檢查尚未結束的 Run 數與未釋放的 lease 數各 ≤1（:204-217）。
- [PASS] 交接不增加修正計數 — 舊 Run 改走 RunCommand::HandOff 結束，呼叫 finish 時 charge 傳 None（crates/baton-domain/src/run.rs:885-896）。情境測試確認沒有 run_failed／run_interrupted 事件、只有 1 筆 run_handed_off，工作面板上三種預算的 used 都是 0（handoff.rs:478-515）。
- [PASS] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票的 AI 決策已追加到 ai-decisions.md — crates/baton-harness/tests/handoff.rs 有 15 個情境測試全部通過，走真實 HTTP API、真實 SQLite，並使用 fake Clock、Adapter、ProcessControl 與 QuotaObserver；Domain 單元測試 handoff::tests 17 個通過。ai-decisions.md 新增 AI-280～AI-290，每條都標了重要度。
- [PASS]（票面 Comments：票 27 額度完整性判斷）— crates/baton-domain/src/quota.rs:438 已改成 `o.used.is_none() && o.status != Some(Exhausted)` 才算缺少已觀測用量，等同主控要求的「`used.is_some()` 或 `status == Exhausted`」才算已觀測。新增測試 crates/baton-domain/src/quota/tests.rs:265 `a_warning_without_a_number_does_not_make_the_window_complete`，斷言判斷用量仍是 0.75，但 complete 為 false、原因含「五小時」，已通過。決策記為 AI-288。
- 前置條件（標註，不影響判定）：驗證開始時 `git merge-base --is-ancestor rebuild/v1 HEAD` 成立（當時 rebuild/v1 = 6ba16c1）。驗證途中整合分支前進到 3562b49（併入票 31，用掉 migration 28 與 AI-280～AI-286），目前已不是 HEAD 的祖先。本票的 migration 28 `quota_gate_and_handoff` 與 AI-280～AI-290 會跟它衝突，合併時要再順延。
TESTS:
- `pnpm test`：結束碼 0（顯示「Rust：通過」「前端：通過」）。Rust 75 個測試執行檔共 1218 passed、0 failed；前端 vitest 18 個檔案、418 passed。
- `pnpm build`：結束碼 0，release 版完成，產出 target/release/baton-desktop.exe。有 1 個 unused import 警告（`StoreError`，crates/baton-api/src/shared_settings.rs:20），這個檔案不在本票 diff 內，是既有問題。
- 驗證結束時 `git status` 乾淨，沒有修改任何追蹤中的檔案。
FIXES_NEEDED: none
```

以下是不影響判定的觀察：

- **重新整合**：合併 rebuild/v1（3562b49，票 31）時，本票的 migration 要順延為第 29 個，AI 編號要從 AI-287 起重新編。
- **Worker 自行結束的判定（AI-283）**：收到 `Exited` 就直接轉移，沒有先用 ProcessControl 確認整個程序樹都結束了。這跟既有一般結束的語意一致，但程序樹裡還留著子程序的話，理論上可能短暫出現第二個 writer。之後可以考慮轉移前先 `inspect`。
