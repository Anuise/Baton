結論：**第 18 張票驗證通過。** 四條驗收條件都成立，Rust 與前端測試全部通過，建置成功，工作樹仍是乾淨的。

另有兩個小問題，不影響驗收：

1. 判斷重送時沒有比對 `correlation_id`。同一個 `message_id` 換一個 `correlation_id` 重送，會被當成重複訊息，回傳的是原本那一則（`crates/baton-domain/src/message.rs:318-323`）。
2. `ai-decisions.md` 的 AI-95～AI-99 插在 AI-94 前面，AI-99 結尾和 `### AI-94` 之間少一行空行，五則也都沒有其他條目有的「審閱：未審」欄位。這只是排版。

```
VERDICT: PASS
CRITERIA:
- [PASS] rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 成功（rebuild/v1 = 1898cb6，HEAD 8c1cfcb 是把它合併進來的 merge commit）
- [PASS] 重複送出的訊息只交付一次 — 去重邏輯在 crates/baton-domain/src/message.rs:318-325：同一 Task、同一 message_id、內容相同時回 Duplicate，不寫入任何資料；內容不同或換了寄件 Task 則回 409 message_id_conflict。資料庫在 crates/baton-store/src/lib.rs 新增的 migration message_and_delivery 以 (workspace_id, message_id, recipient_task_id) 為主鍵，同一收件人不可能有第二份。情境測試 crates/baton-harness/tests/messages.rs:276（多收件人、收件人順序不同、四種內容變體、別的 Task 用同一 id）與 :330（寄件 Run 換世代後重送仍算重複）都通過，訊息與交付數量不變。
- [PASS] Worker 不能讀取不屬於自己範圍的訊息 — crates/baton-store/src/messages.rs:126-191 的 read_inbox 只用憑證推出的 Task 查詢（WHERE recipient_task_id = 憑證 Run 的 task）；InboxRequest 沒有 task_id 參數，聲稱別的 workspace／run 會被 scope_of 以 403 agent_scope_mismatch 拒絕（crates/baton-api/src/agent.rs:88-103）。回覆只能指向自己寄過或收到的訊息（message.rs:328-338）。情境測試 messages.rs:354（別的 Task、聲稱別的 run／workspace、帶 task_id 欄位）、:404（跨 Workspace 一律回 404 recipient_not_found，不洩漏資料）、:435（桌面憑證回 403 role_not_allowed）、:745／:782（被接替或已結束的 Run 回 401 credential_revoked，且不改變已讀狀態）都通過。
- [PASS] 嘗試以訊息變更範圍時被忽略，並轉為給 Controller 的事件 — classify 在 crates/baton-domain/src/message.rs:192-203 偵測 5 種 kind 與 8 種欄位；API 回 200 {accepted:false, ignored:true, reason:"scope_change_not_allowed"}（crates/baton-api/src/messages.rs:127-140）。Store 不儲存訊息，只追加 message_scope_change_ignored 事件（crates/baton-store/src/messages.rs:101-121）。注意 Claims 只會取走 workspace_id 和 run_id，所以 task_id 會留在 extra 裡被偵測到。情境測試 messages.rs:457 涵蓋 14 個案例，逐一確認事件 payload、訊息與交付數為 0，兩邊 Task 的契約與詳細資料都沒變。
- [PASS] 以主要 seam（Runtime harness，D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加 — crates/baton-harness/tests/messages.rs 共 18 個情境測試，全部通過（測試記錄中 "18 passed; 0 failed"），涵蓋交付、去重、範圍、範圍變更事件、Run 啟動時的未讀摘要（附加在 crates/baton-store/src/runs.rs:58-60 處理派工的同一個交易；全程式只有 Dispatch 會產生 Outbox，所以每個 Run 啟動都會經過這裡）、對話串、世代、MCP shim 與重啟。ai-decisions.md 新增 AI-95～AI-99（重要度：中、中、高、中、低），diff 只有新增、沒有修改既有條目。
TESTS:
- `pnpm test`（= cargo test --workspace --exclude baton-desktop 加上 pnpm --filter web test）：Rust 全部通過，加總各 test result 為 487 passed, 0 failed。前端第一次失敗，原因是 worktree 沒有 node_modules（'vitest' 不是可執行的命令），屬於環境問題。
- `pnpm install --frozen-lockfile` 後執行 `pnpm --filter web test`：Test Files 10 passed，Tests 128 passed。
- `pnpm build`（release 版 baton-runtime 加上 tauri build --no-bundle）：EXIT=0，產生 target/release/baton-desktop.exe。
- 執行後 `git status --porcelain` 為空，沒有改動任何追蹤中的檔案。
FIXES_NEEDED: none
```
