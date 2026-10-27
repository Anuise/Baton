VERDICT: PASS
CRITERIA:
- [PASS] 前置條件：`rebuild/v1` 是 HEAD 的祖先。驗證開始時 `git merge-base --is-ancestor rebuild/v1 HEAD` 成立，當時 rebuild/v1 = 9105144。**驗證途中整合分支前進到 fa8d7e5（票 45 已合併），目前已不是祖先。** 依指示只標註，不據此判 FAIL；合併前需要處理的事項列在下方。
- [PASS] 需求符合但規範違反、規範符合但需求缺漏，兩者都不能通過 Gate。
  - `crates/baton-domain/src/review.rs:378-386`：每一軸的 finding 必須全部經使用者同意駁回，或這一軸是沿用的，該軸才算通過；兩軸都通過，審查才算通過。
  - `crates/baton-store/src/reviews.rs:266-290`：Gate 在交易中查驗，未通過就回報 `not_passed`。
  - 內建 feature Playbook 的「獨立審查」改為要求 `Review` 證據（`workflow/builtin.rs:300`）。
  - 情境測試 `two_axis_review.rs:234-304` 分別送出「Standards 不通過／Spec 通過」與「Standards 通過／Spec 不通過」，兩者都得到 `incomplete`／`not_passed`，Stage 停在 `independent_review`。駁回 finding 必須經使用者裁定，Controller 再呼叫一次才套用；之後才轉為 `passed`。
  - Domain 測試 `the_independent_review_gate_needs_a_passed_two_axis_review_not_an_artifact` 也確認：用成果代替審查會被拒絕，回 `workflow_invalid`。
- [PASS] 作者不能擔任自己工作的必要 reviewer。
  - `review.rs:409`：被審 Task 本身，或與作者的 Session 有重疊，都回 `ReviewerIsAuthor`。
  - `store/reviews.rs:178-196`：作者 Session 涵蓋 agent、native 與 resume 三種。
  - Gate 依每一軸「實際審查的 reviewer」去重後計數（`workflow.rs` 的 `review_shortfall`）；人數不足記為 `blocked`。
  - 情境測試 `two_axis_review.rs:309-375` 涵蓋：
    - 作者 Run 提交審查 → 403 `reviewer_is_author`；
    - Controller 提交 → 403 `worker_only`；
    - 沒有 reviewer → `blocked`，原因是「需要 2 位，目前 0 位」；
    - 只有一位 reviewer → `blocked`；
    - 同一位 reviewer 交兩份 → 仍是 `blocked`；
    - 審查的是別的 Task → `other_subject`；
    - 第二位非作者 reviewer 通過後 → `passed`。
  - Domain 測試另外確認：只沿用第一位 reviewer 的某一軸，不算第二位。
- [PASS] 修正後只有可證明不受影響的審查結果被沿用。
  - `review.rs:503-557` 依序檢查：同一個被審 Task、該軸已通過、規格版本或規範來源相同、有記錄審查範圍（paths），以及 Runtime 算出的變更檔案沒有落在範圍內（比對不分大小寫，並含其下的目錄）。
  - 變更檔案在 `api/reviews.rs:290-305` 由 Runtime 以 Git 計算，不採信自報。正式實作是 `git diff --name-only --no-renames --end-of-options`（`baton-ports/src/git.rs`），並設定 `core.quotepath=false`。
  - 情境測試 `two_axis_review.rs:380-445` 涵蓋：
    - 該軸還有待修正的 finding → `review_reuse_rejected`；
    - 變更命中審過的範圍（含大小寫不同與反斜線寫法的 `SRC\Export.rs`）→ 拒絕；
    - revision 不存在 → 回「無法證明」並拒絕；
    - 未受影響的 Spec 軸沿用成功，並記原本的 reviewer；
    - 契約改到第 2 版後：舊審查變成 `stale`，Spec 軸不能沿用，Standards 軸可以沿用。
- [PASS] 以 Runtime harness（D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加。
  - `crates/baton-harness/tests/two_axis_review.rs` 的 4 個測試全部通過，包含設計偏離：保存原因與接受者 `desktop`、設計更新到第 2 版、下一位 Worker 的 context receipt 拿到 version 2。
  - `ai-decisions.md` 新增 AI-391～AI-398，都標有重要度。
- [PASS] 票 43 情境測試的改寫沒有弱化斷言。整個 diff 只改了 `methods_playbooks_stages.rs:425-441` 一處。
  - 原本的 `pass_with_artifact` 換成「非作者 Worker 提交雙軸審查 → 201 → 以 `review_id` 回報 → 斷言 `outcome == passed`」。
  - 前後新增的 `current_stage` 斷言是 `independent_review` → `integration`。
  - 原有斷言全部保留，包括後續 integration 不可跳過、`not_accepted` 等。
  - `workflow/tests.rs` 只有新增，沒有刪除任何斷言；mcp-shim 的工具數從 26 改成 31，同步了 5 個新工具。

TESTS:
- `pnpm test`：結束碼 0。
  - Rust：`cargo test --workspace --exclude baton-desktop` 共 1530 個 ok、0 failed；ignored 的只有需要真實已登入 Claude Code 的 live 測試。
  - 前端：vitest 28 個檔案、589 個測試全部通過。
- `pnpm build`：結束碼 0，產出 `target/release/baton-desktop.exe`、`baton-runtime.exe` 與 `baton-guardian.exe`。只有一個警告：`crates/baton-api/src/shared_settings.rs:20` 的 `StoreError` 沒有用到。本票沒有改這個檔案，是既有的警告。
- 工作樹保持乾淨（`git status` 沒有任何變更）。

FIXES_NEEDED: none

本票功能判定為 PASS，但下次整合前實作者必須先處理整合分支前進帶來的問題。這不是本票功能的缺陷：
1. `git merge-tree HEAD rebuild/v1` 顯示 14 個檔案有內容衝突，包括 `baton-api/src/lib.rs`、`baton-domain/src/workflow.rs`、`baton-domain/src/workflow/tests.rs`、`baton-mcp-shim/src/lib.rs` 與 `tests/scenario.rs`、`baton-store/src/lib.rs`、`baton-store/src/workflows.rs`、`docs/development.md` 與多個 `web/src/*`。要重新合併 rebuild/v1 並解決衝突。
2. 編號重複：票 45 已在 rebuild/v1 使用 migration 40（`verification_profiles`）與 AI-391～AI-398。本票的 migration `reviews_and_deviations` 要改成 41，AI 決策要順延成 AI-399 起編。AI-398 已預告這種情況。
3. mcp-shim 的工具總數斷言與 `CONTROLLER_TOOLS` 陣列長度，要依合併後的實際工具數更新。
4. 合併後要重跑 `pnpm test` 與 `pnpm build`。

另有一個低重要度的觀察，不影響判定：即使設定 `core.quotepath=false`，git 遇到檔名含 `"`、`\` 或控制字元時仍會把路徑加上引號。這時 `touches` 比對會漏判，把受影響的軸當成可沿用。可以改用 `git diff -z` 修正，留給後續處理即可。
