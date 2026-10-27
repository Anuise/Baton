```
VERDICT: PASS
CRITERIA:
- [PASS] 前置條件：rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 回傳 ANCESTOR_OK。驗證時 rebuild/v1 停在 263efdb，HEAD 4a75c85 已合併它。
- [PASS] 時間線依 Task 過濾後事件完整且有序 —
  ・事件分類寫在 crates/baton-domain/src/timeline.rs:96-120。票面要求的派工、交接、裁定、恢復、驗證、清理六類都有，另外加了 Task 狀態、外部操作、教訓重犯、用量四類，供指標計算使用。依 Task 過濾的函式在 timeline.rs:151-153，只篩選，不改變順序。
  ・讀取邏輯在 crates/baton-store/src/timeline.rs:76-139。整條時間線依 `ORDER BY e.seq` 排序。每筆事件依它所屬的聚合找出相關 Task：Run、交接、裁定請求從各自的資料表取；Task 與清理事件的聚合識別就是 Task；其他事件從 payload 的 task_id 或 run_id 推出。恢復事件改由 recovery_attempt 處置過的 Task 決定。別的 Workspace 的事件會被排除。
  ・Harness 情境測試 `filtering_the_timeline_by_task_keeps_every_event_of_that_task_in_commit_order` 在 crates/baton-harness/tests/timeline_and_metrics.rs:278-342。它驗證三件事：seq 嚴格遞增；過濾結果 == 全部事件中 concerns(task) 的子序列；研究 Task 的整段經過依序出現，從裁定請求、回答、交接、重複操作被擋，到崩潰恢復與教訓重犯。
  ・`a_task_outside_the_workspace_cannot_be_used_as_a_filter`（:344-364）：拿別的 Workspace 的 Task 過濾回 404 task_not_found，也不會讀到其他 Workspace 的事件。
- [PASS] 每項指標可由事件重新計算並與顯示一致 —
  ・`baton_domain::metrics::compute` 在 crates/baton-domain/src/metrics.rs:138-301，是純函式，只吃時間線事件。API 會先取一次時間線，再拿同一批事件呼叫它（crates/baton-api/src/timeline.rs:93-117）。
  ・Harness 測試 `every_metric_is_recomputed_from_the_timeline_events_and_matches_what_is_shown`（:366-398）斷言 `metrics::compute(GET timeline) == GET metrics`，並逐項核對實際數值：完成率 1/2、交接 1/1、恢復 5 秒、重複操作 1、裁定等待 90 秒、重犯率 1/3、用量 1500、Context 已回報 1 個 Run、未回報 1 個 Run。它也驗證 quota_usage 過了保留期被刪除後，指標仍然不變。
  ・票面列的指標都有：工作完成率、交接成功率、恢復耗時、重複操作事件、裁定等待時間、教訓重犯率、每階段用量、Context 使用量。
  ・額外的服務端額度：看不到時 API 回 null，UI 顯示「未知」。對應 timeline.rs:121-128、TimelinePage.tsx:79-81，以及 harness 測試 `server_side_quota_that_cannot_be_seen_is_unknown_even_when_local_usage_is_estimated`（:435-469），涵蓋「有本機推估但沒有服務端數字時仍是 null」。
- [PASS] 以主要 seam（Runtime harness，D-04）撰寫的情境測試涵蓋本票行為並通過；本票的 AI 決策已追加至 ai-decisions.md —
  ・5 個 harness 情境測試全部 ok，測試輸出 /tmp/r114-test.log:1479-1483。它們透過真實 Axum API 執行，搭配 fake Clock、scripted AgentAdapter，並模擬 crash_restart。
  ・ai-decisions.md 新增 AI-327～AI-334 共 8 條，每條都有重要度（中 5 條、低 3 條）。
  ・UI 是 web/src/TimelinePage.tsx，入口是側欄的「時間線」按鈕。前端測試涵蓋「無資料」不顯示成 0，以及服務端額度「未知」（TimelinePage.test.tsx:112-154）。
TESTS:
- `pnpm install --frozen-lockfile` → exit 0
- `pnpm build` → exit 0，產出 target/release/baton-desktop.exe 等。只有 1 個 warning：baton-api/src/shared_settings.rs:20 的 unused import `StoreError`。這個檔案不在本票 diff 內，是既有的問題。
- `pnpm test`（= `cargo test --workspace --exclude baton-desktop` + `pnpm --filter web test`）→ exit 0
  ・Rust：1355 passed / 0 failed / 8 ignored（皆為既有的真實 Adapter 契約測試）
  ・前端：Test Files 21 passed，Tests 478 passed
- 驗證結束後 `git status` 乾淨，沒有修改任何追蹤中的檔案。
FIXES_NEEDED: none
```

以下幾點不影響判定，提供後續參考：

1. **每次查詢都掃整張事件表**：`read_timeline` 每次都讀全庫的 TIMELINE_AGGREGATES 事件與全部 task，過濾是在 Rust 端做。事件量大時會變慢。
2. **恢復耗時不是單一 Workspace 的數字**：恢復事件屬於整個 Runtime，會出現在每個 Workspace 的時間線上。所以各 Workspace 顯示的恢復耗時其實是全 Runtime 的統計（AI-327 已說明）。
3. **擋多個 Task 的裁定請求只歸屬到一個 Task**：裁定請求只關聯到發起它的 Task，被 `decision_request_block` 一起擋住的其他 Task，在它們自己的時間線上看不到這個裁定。
4. **同一個 Run 的重複登記也會計入**：同一個 Run 對已成功的冪等鍵再登記一次，也會記為重複操作事件。這跟「另一個 Run 被擋下」的定義略有出入，但影響很小。
