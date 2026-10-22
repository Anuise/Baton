VERDICT: FAIL

票 20 的四條驗收條件都通過，建置與全部測試也都過了。判 FAIL 只有一個原因：驗證期間整合分支 `rebuild/v1` 併入了票 14，所以 `rebuild/v1` 已經不是 HEAD 的祖先，而且兩邊有衝突，現在不能直接併回。

```
VERDICT: FAIL
CRITERIA:
- [FAIL] rebuild/v1 是 HEAD 的祖先 — 驗證開始時 rebuild/v1=c4bd038，`git merge-base --is-ancestor` 回 ANCESTOR_OK；驗證途中 rebuild/v1 前移到 c99cb26（afa453f「merge: 票 14」、64940f4、c99cb26），重跑得到 NOT_ANCESTOR。`git merge-tree --write-tree HEAD rebuild/v1` 試合併（不動工作目錄）有 4 個衝突：crates/baton-domain/src/run.rs、crates/baton-runtime/src/production_ports.rs、crates/baton-store/src/lib.rs、src-tauri/src/main.rs。另外有兩處編號重複：(a) 票 14 的 `model_overrides_and_native_session` 和本票的 `task_workdir_and_integration_queue` 都是第 19 個 migration（crates/baton-store/src/lib.rs）；(b) ai-decisions.md 會自動合併，但兩邊都有 AI-200～AI-206 的標題（票 14 用到 AI-207）。
- [PASS] 不同開發 Task 各自擁有 worktree，與 Session／Run 關聯正確 — crates/baton-harness/tests/worktrees.rs:151-190：兩個開發 Task 的 Worker 分別在 `<ws>\.baton\worktrees\<task>` 啟動，分支是 `baton/<task>`，目錄真的有建立。工作面板登記了 kind=worktree、branch、base_branch=main、run_ids=[r1]。失敗後重新派工會回到同一個 worktree，run_ids=[r1, again]，version 仍是 1（不重建）。worktrees.rs:215-251 涵蓋目錄不可用的情況：不派工，回 202 並帶 category=workdir 與原因（「不是 Git repo」／「檔案被占用」），60 秒後自動重試派出。真實 git 實作在 crates/baton-ports/src/git.rs:28-48，有實際執行 git 的單元測試（baton_ports 16 passed）。
- [PASS] 整合佇列一次只處理一個合併；合併後驗證失敗時 Task 不完成 — worktrees.rs:256-321：第一個合併停住時，佇列 active 是第一個（merging），第二個是 queued，兩個 Task 都還在 verifying。放行後依序合併，第二個以第一個的合併結果為基底，main 前移到最後的結果，每個項目在整合目錄各跑一次整合驗證，最後都 completed。DB 層有唯一索引 `integration_entry_one_active`（crates/baton-store/src/lib.rs migration 19）。worktrees.rs:324-346：整合驗證失敗時 Task 回到 ready（不是 completed），main 留在 base，證據存進項目的 checks／failure。worktrees.rs:349-367：合併衝突時判失敗，不跑整合驗證。worktrees.rs:370-397：手動接受也要等整合完成。worktrees.rs:400-424：合併途中崩潰，重啟後只整合一次。狀態機在 crates/baton-domain/src/integration.rs:254-361。
- [PASS] 非 Git Workspace 的研究 Task 使用輸出目錄而非 worktree — worktrees.rs:193-212：研究 Task 在 `<ws>\.baton\outputs\<task>` 執行，目錄真的有建立，`has_worktree` 為 false，登記 kind=output_dir、branch 與 base_branch 為 null。文件 Task 有自己的輸出目錄。
- [PASS] 以 Runtime harness 撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加至 ai-decisions.md — crates/baton-harness/tests/worktrees.rs 共 8 個測試，全部透過真實 HTTP API 操作（8 passed）。本票的決策記在 ai-decisions.md 的 AI-196～AI-206（以 c4bd038 為準編號，見第一條）。docs/development.md 的 harness 一節也補上了新的 fake 腳本 API。
TESTS: `pnpm test`（在 HEAD 59ffac2 上；= with-msvc cargo test --workspace --exclude baton-desktop + pnpm --filter web test）→ EXIT=0。Rust 共 64 個測試執行檔全部 ok、0 failed（baton_domain 359、baton_store 83、harness worktrees 8 等）；前端 Test Files 17 passed、Tests 327 passed。`pnpm build` → EXIT=0，release 版產出 target\release\baton-desktop.exe。工作目錄在建置與測試後仍是乾淨的（git status 沒有輸出）。
FIXES_NEEDED:
1. 把目前的 rebuild/v1（c99cb26，含票 14）合併進 rebuild/20-worktrees-and-integration-queue，解決下列 4 個衝突：
   - crates/baton-domain/src/run.rs：保留票 14 的 RunSpec／原生 Session 綁定，和本票的工作目錄欄位。
   - crates/baton-runtime/src/production_ports.rs：保留票 14 的 `production_ports(data_dir)`，和本票的 Git WorkspaceFs 實作。
   - crates/baton-store/src/lib.rs：保留兩邊的 migration。
   - src-tauri/src/main.rs：合併兩邊的匯入。
2. 本票的 migration `task_workdir_and_integration_queue` 改成第 20 個（排在票 14 的 `model_overrides_and_native_session` 之後），並同步更新 migration 註解、ai-decisions 與開發指南中「第 19 個 migration」的字樣。
3. ai-decisions.md 中本票的條目從 AI-196～AI-206 順延到 AI-208 之後（rebuild/v1 已經用到 AI-207），條目內的交互引用一起改，並追加一則決策記錄這次整合。
4. 整合後重跑 `pnpm test` 與 `pnpm build`，確認都通過，並確認 `git merge-base --is-ancestor rebuild/v1 HEAD` 成立。
```
