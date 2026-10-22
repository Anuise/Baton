所有驗收條件都通過，建置與全部測試也都在前景跑完。整合分支在驗證途中前進了，詳見下方前置條件。

```
VERDICT: PASS
CRITERIA:
- [PASS] 相關的已驗證教訓在 Task 開始前被載入 — 派工時就組好 Prompt，在同一個交易裡寫回派工 Outbox。四條派工路徑都改成呼叫 attach_context：一般派工 crates/baton-store/src/runs.rs:82、排程派工 scheduling.rs:175、交接 handoffs.rs:388 與 :528（舊的 attach_unread_summary 已全部移除）。只撈 status='verified' 的條目（knowledge.rs load_verified_guidance），Domain 再檢查有效期間（is_effective_at）與適用範圍（Audience::Run.can_see），見 crates/baton-domain/src/context.rs:219-240。情境測試 crates/baton-harness/tests/context_builder.rs:144-184 用 fake AgentAdapter 收到的啟動 Prompt 確認：已驗證教訓有載入；已撤銷的、候選中的、其他 Task 範圍的都沒載入；候選經使用者確認後，之後的 Run 才載入；已啟動 Run 的 Prompt 不變。
- [PASS] 預算不足時省略項目被記錄於 receipt — context.rs:189-204 依優先序逐項放入，超出預算的項目標記為 status=Omitted、omitted_reason=BudgetExceeded，後面較小的項目仍會放入。Task 契約與交接說明一定載入（context.rs:150-166）。情境測試 context_builder.rs:187-216 驗證：被省略的那則教訓在 receipt 中記為 omitted 與 budget_exceeded，並有 kind、version、tokens；budget_tokens 等於 8000；used_tokens 不超過預算；省略的內容不在 Prompt 中。
- [PASS] receipt 版本與實際載入內容一致 — Prompt 與 receipt 由同一次 build() 產生（context.rs:150-216），receipt 帶 prompt_sha256。receipt 存在 context_receipt 表（migration lib.rs，只追加，有禁止 UPDATE／DELETE 的 trigger）。API GET /api/v1/workspaces/{id}/runs/{run_id}/context 回傳 receipt 加上 Outbox 中固定的那份 Prompt（store context.rs:102-140）。情境測試 context_builder.rs:219-260 驗證：之後的修正不會改變已保存 receipt 的版本（v2 對 v3）、Prompt 只含該版本的做法、雜湊相符、API 的 Prompt 等於 Worker 實際收到的 Prompt、重啟 Runtime 後 receipt 不變。另外 assert_receipt_matches（:129-139）逐項核對：已載入的條目以 receipt 記錄的版本出現在 Prompt 中，省略的不出現。
- [PASS] 以主要 seam（Runtime harness，D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加至 ai-decisions.md — crates/baton-harness/tests/context_builder.rs 共 5 個情境測試全部通過（透過真實 HTTP API 與 fake Port 執行）；另有 Domain 單元測試 context::tests 13 個全部通過。ai-decisions.md 已追加 AI-350～AI-354（第 3154 行起），每則都標有重要度（中／中／低／中／低）。Run 詳情的查看功能：src-tauri 的 run_context 命令已登記（main.rs:1101），Web UI 在 TaskBoard.tsx:826 呼叫並有對應測試。
TESTS:
- `pnpm test`（cargo test --workspace --exclude baton-desktop，加上 pnpm --filter web test）→ EXIT=0。Rust 測試全數通過（0 failed；context_builder 5/5，baton-domain 591 passed），前端 21 個檔案、488 個測試全數通過。
- `pnpm build` → EXIT=0，產出 target/release/baton-desktop.exe。有一個 unused import 警告（crates/baton-api/src/shared_settings.rs:20），rebuild/v1 上本來就有，不是本票造成的。
- 驗證過程沒有改動任何追蹤中的檔案（git status 乾淨）。
- 前置條件：一開始 `git merge-base --is-ancestor rebuild/v1 HEAD` 成立（rebuild/v1 當時是 ee20f02）。驗證途中 rebuild/v1 前進到 172280b，多了一個 commit「chore(scratch): 記錄 R110，準備 R120」，只改了 .scratch/.orchestration 底下的 runs.md 與 R120 的 prompt 檔，沒有程式碼變更。因此目前 rebuild/v1 不再是 HEAD 的祖先，依指示只標註、不判 FAIL；整合前需要再合併一次。
FIXES_NEEDED: none
```
