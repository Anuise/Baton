# 最終 review 待修清單

1. 可能有不穩定的 Rust 測試：R005（票 01 整合）第一次 `pnpm test` 的 Rust 部分回結束碼 101，輸出未保存；之後三次 `cargo test --workspace --exclude baton-desktop` 都通過。最終 review 時留意是否有依賴時序或共用資源的測試。
2. 不穩定測試（承第 1 項）：R011（票 07 實作）整套 `cargo test` 第一次時 `verification_handoff::after_a_crash_the_successor_stops_the_old_test_processes_and_reruns_the_unfinished_case` 失敗一次，單獨重跑 3 次與第二次整套都通過。可能就是第 1 項那個失敗。
3. 不穩定測試：R009（票 05 實作）高負載整包跑時 `baton-store` 的 `lifecycle::tests::concurrent_writers_of_the_lifecycle_file_do_not_trip_over_each_other` 失敗一次，單獨重跑 3 次與整包重跑都通過。
4. 操作手冊（票 04）要寫明 `"$BATON_MCP_SHIM" wait` 的用法與結束碼 0～4（票 06 的 AI 決策；R014 驗證者提出）。票 04 與 06 平行開發，若票 04 合併時手冊還沒寫，最終修正要補上。
5. 不穩定測試（承第 1～3 項）：R008（票 04 實作）高負載下另見 `worktree_cleanup.rs` 的當掉清理測試、`worktrees.rs` 的當掉合併測試各偶發失敗；R020、R016 都再遇到第 3 項的 lifecycle 並行寫入測試（「存取被拒 os error 5」）。最終修正應找出原因並讓它們穩定，而不是放寬斷言。
6. 開啟 Controller CLI 時 `supervisor/controller_cli.rs` 仍呼叫 `agent_adapter.prepare`，在 `runs\<controller_run_id>\` 寫出一份用不到的 Worker 通則 `system-prompt.md`（R020 觀察，低）。
7. 票 08 刻意暫留的對話殘留（R021 觀察）：`crates/baton-domain/src/decision.rs` 的 `DecisionChange.notices`／`ConversationNotice` 仍會產生但沒有讀者，`SessionOwner::Conversation`、`WorkflowNote` 在正式程式碼已無使用者，`decision.rs` 多處註解仍寫「喚醒 Controller」。票 06 已合併且沒有沿用，最終修正可清掉（AI 決策：票 08 的「通知型別暫留」條目）。
8. 前端不穩定測試：R019 加壓（3 套同時跑 ×4 輪）時，`ExplorationPage` 與 `ControllerPage`（票 03 的既有測試）的 `findBy` 在高負載下逾時或時序太早，各失敗一次。
9. `Store::controller_cli_accounts`（`crates/baton-store/src/controller_cli.rs`，票 05）目前沒有呼叫端（R019 觀察）。
10. 已要求換帳號、宿主尚未結束時 Runtime 當掉，重啟後宿主已不在 → Run 走 interrupted，換帳號要求不會開出新 Run（R023 觀察，極端時序，spec 未規定）。

## 最終修正的分派（主控，2026-10-23）

- 第 4 項已由票 04 完成（操作手冊已寫 `"$BATON_MCP_SHIM" wait` 與結束碼，R020 驗證），不必再處理。
- **final-ab**（Rust 後端：Domain、Store、Runtime、API、Ports、MCP shim、Client）：第 3 項（`baton-store` lifecycle 並行寫入測試，含第 5 項中的同一支）、第 6、7、9、10 項。
- **final-c**（Web、Tauri、scripts、docs、GLOSSARY、harness）：第 1、2、5 項（harness 的當掉模擬測試；R028 第 1 項已找出成因）、第 8 項。
