VERDICT: FAIL

所有功能驗收條件都通過，建置和全部測試也都通過。判 FAIL 只有一個原因：驗證期間 `rebuild/v1` 併入了票 25，現在這個分支直接合併會衝突，AI 條目編號也會重複。修正只需要重新整合分支，不用改功能。

CRITERIA:
- [FAIL] 前置：`rebuild/v1` 是 HEAD 的祖先 — 實作者在 `cca0ec4` 合併的 `rebuild/v1` 是 `6edd06c`。驗證開始時 `rebuild/v1` 已經前進到 `b81e5e3`（只改了 `.orchestration/runs.md`），驗證期間又併入票 25（`045150b`），目前是 `1868fc4`。
  - `git merge-base --is-ancestor rebuild/v1 HEAD` 回傳 exit=1。
  - `git merge-tree --write-tree HEAD rebuild/v1` 回報 `CONFLICT (content): Merge conflict in docs/development.md`。
  - 票 25 在整合分支已經用掉 AI-168～AI-179，本票新增的 AI-168～AI-172 會跟它重號。
- [PASS] 相同操作在換帳號或 Run 後仍可沿用批准，參數或成果版本改變時拒絕
  - `crates/baton-harness/tests/one_off_approval.rs:213-232`：讓第一個帳號額度吃緊後，新的 Run 派給另一個帳號，用同一個操作（只改說明文字）提示，結果為 `approved`，並記下 `approval_use.run_id`。
  - `:235-256`：`--force` 回 `params_changed`，`origin release` 回 `target_changed`。被拒絕的嘗試不會用掉批准，原本的操作之後仍可執行一次。
  - `:259-283`：登記新的成果版本後回 `artifact_version_changed`，並建立新請求。
  - 實作：`crates/baton-domain/src/approval.rs` 的 `bind`／`check` 綁定五項內容，裡面不含 Run、帳號或時間；`crates/baton-domain/src/decision.rs` 在 permission_prompt 時逐項核對。
- [PASS] 同一批准不能用於第二次操作；結果未知時先查證
  - `one_off_approval.rs:288-308`：第二次提示回 `used_result_unknown`，新請求的問題含「結果未知」，帶 `prior_use`（哪個批准、哪個 Run 用的），建議 `deny`，選項為「已查證，允許再執行一次」。
  - 防止重複使用：讀取、判定、寫入在同一個寫入交易內（`crates/baton-store/src/decisions.rs:146-148`），`decision_approval_use` 是追加式資料表（`baton-store/src/lib.rs:522-530`）。
- [PASS] 單次批准不增加 Workspace 授權
  - `one_off_approval.rs:313-331`：使用批准前後 `revisions` 相同，生效授權仍然沒有 `remote_write`，另一個 Task 的同一個操作仍要裁定。
  - 修改授權另外要求明確確認：`crates/baton-domain/src/authorization.rs` 的 `confirm` 在擴大授權但沒帶 `acknowledged_expansion` 時回 400 `authorization_expansion_not_acknowledged`（`one_off_approval.rs:359-405`）。
  - UI 把兩件事分成兩個區塊：`web/src/DecisionCenter.tsx:232-236`（允許本次操作）、`:432-474`（修改 Workspace 授權），另有 10 個新增的 vitest 情境。
- [PASS] 以主要 seam（Runtime harness，spec D-04）寫的情境測試涵蓋本票行為並通過；AI 決策已追加至 ai-decisions.md
  - `one_off_approval.rs` 共 7 個測試，都透過真實的 Axum API、Agent 與桌面兩組憑證、fake Clock 和 fake QuotaObserver：`test result: ok. 7 passed`。
  - ai-decisions.md 已追加 AI-168～AI-172，各有重要度（高／高／高／中／低），但編號與票 25 衝突，見第一條。

TESTS:
- `pnpm test`：exit=0，約 1 分 44 秒。
  - Rust 全部 crate 通過：baton-domain 311、baton-store 69、one_off_approval 7、decisions 20、workspace_authorization 19、tasks 54、scheduling 32，其餘測試檔全部 ok，0 failed（baton-runtime 有 3 個預設忽略的 live 測試）。
  - 前端 vitest：15 個測試檔、282 個測試全數通過。
- `pnpm build`：exit=0，約 2 分 40 秒，產出 `target/release/baton-desktop.exe`。
- 跑完後 `git status` 是乾淨的，沒有修改任何追蹤中的檔案。

FIXES_NEEDED:
1. 把目前的 `rebuild/v1`（`1868fc4` 或更新的版本，含票 25）合併進 `rebuild/23-single-operation-approval-e1`，然後確認 `git merge-base --is-ancestor rebuild/v1 HEAD` 成立。
2. 解決 `docs/development.md` 的衝突。比照 AI-172 的做法，crate 表和測試清單兩邊都保留：本票的 `approval.rs`、`one_off_approval.rs`，以及票 25 的 lifecycle 相關項目。
3. 本票的 AI 條目從 AI-168～AI-172 順延為 AI-180～AI-184，只改編號，並同步更新開發指南和程式註解裡的引用（用 `git grep "AI-16[89]\|AI-17[0-2]"` 找出本票的引用）。依規定，整合分支既有的 AI-168～AI-179 不能動。在 AI-184（原 AI-172）記下這次整合，或另外追加一條。
4. 檢查票 25 新加的程式和測試有沒有建構 `ConfirmRequest`，或擴大既有授權的地方。前者要補 `acknowledged_expansion`，後者的 PUT 要帶 `acknowledged_expansion: true`。
5. 重跑 `pnpm test` 和 `pnpm build`，兩者都要通過。

不阻擋的觀察，可以不修：沒有子命令的 Bash 指令（例如 `rm a.txt`）會把第二個字詞算進操作識別，所以換目標時回報的原因是 `operation_changed`，不是 `target_changed`。結果一樣會拒絕沿用，只是原因代碼不夠精確。
