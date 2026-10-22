**VERDICT: FAIL** — 只差一個缺陷：重現還在排隊時，Controller 可以先把疑點排除；之後重現確認是產品缺陷，疑點仍掛在「已排除」。建置、全部測試和另外三條驗收條件都通過。

```
VERDICT: FAIL
CRITERIA:
- [PASS] 前置條件：rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 成立（rebuild/v1 = e731e17，HEAD 的合併 commit 03cc987 已含它）。
- [PASS] 環境故障不被記為產品缺陷 — 只有「案例以產品行為斷言失敗（product_failure）而且對照的代表旅程全部通過」才確認；其他判定、案例沒有執行或對照沒通過，結果都是 Inconclusive，狀態不變（crates/baton-domain/src/exploration.rs:728-758）。情境測試 `an_environment_failure_is_never_recorded_as_a_product_defect`（crates/baton-harness/tests/exploration_to_reproduction.rs:304-351）涵蓋服務沒起來、端點連不上、401 授權阻礙、對照本身失敗四種情況，疑點一直維持 suspected。測試通過。
- [PASS] 未經重現的疑點不會顯示為 confirmed — 回報一律是 suspected（exploration.rs:372-424），沒有任何分流動作能直接改成 confirmed。資料庫的 CHECK 要求「缺陷狀態 ⇔ 有重現案例」（crates/baton-store/src/lib.rs migration `exploration_findings`）。測試 `a_suspicion_is_only_shown_as_confirmed_after_a_reproduction_fails_on_a_product_assertion`（:356-399）涵蓋直接改資料庫被拒、Worker 不能呼叫、只有人工步驟的案例被拒。UI 把「已確認缺陷／疑點（未經重現）／未驗風險」分區顯示（web/src/ExplorationPage.tsx:29-64）。
- [FAIL] 不能以排除標籤或刪除案例掩蓋缺陷 — 已確認的缺陷確實不能排除、降級或換案例，刪除也被資料庫 trigger 擋下；已修復的案例會強制加進每一次一般驗證（store/exploration.rs:229-260），測試 :404-495 通過。但有一個缺口：重現還在排隊時可以先排除疑點。triage 沒有檢查是否有排隊中的重現（domain exploration.rs:456-502、store triage_exploration_finding）；重現結果出來時，judge_bundle 只在狀態仍是 suspected/unverified_risk 時才更新（store/exploration.rs:344-353）。我用一個臨時、未追蹤的測試實際跑過（跑完已刪除，git status 乾淨）：shop(160) → reproduce_exploration_finding 回 201 → 排除為 environment 回 200 OK → supervise。結果是 `status="excluded" attempt_outcome="reproduced" control="passed" verdict="product_failure"`。也就是說，Runtime 已經證實是產品缺陷，疑點卻還在「已排除（環境）」，UI 也不會把它放進已確認缺陷。
- [PASS] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加至 ai-decisions.md — crates/baton-harness/tests/exploration_to_reproduction.rs 的 4 個測試全部 ok；ai-decisions.md 新增 AI-431～AI-437，都標了重要度（高 2、中 3、低 2）。
TESTS:
- `pnpm test` → exit 0。Rust 測試合計 1637 passed、0 failed、8 ignored；前端（vitest）30 個測試檔、632 個測試全部通過。
- `pnpm build` → exit 0，產出 target/release/baton-desktop.exe 等檔案。唯一的警告是 baton-api/src/shared_settings.rs:20 有 `StoreError` 沒用到；那個檔案不在本票的 diff 內。
- 臨時探針：`cargo test -p baton-harness --test zz_r148_probe r148_probe`（用 scripts/with-msvc.ps1 執行）→ 1 passed，並印出上面那組狀態，證實缺口存在。探針檔已刪除。
FIXES_NEEDED:
1. 疑點有排隊中的重現（purpose 是 reproduce 或 verify_fix、outcome 為 NULL）時，triage 的 Exclude（建議連 MarkUnverified 一起）必須拒絕，回 409 `exploration_reproduction_pending`。要在 store 的 `triage_exploration_finding` 寫入交易內檢查，這樣「先檢查、後寫入」中間不會被插隊。也要擋住 API 層在 expected_design 情況下先建立裁定請求。另一個做法：judge_bundle 遇到 Reproduce 的結果是 Reproduced、疑點卻已是 Excluded 時，仍然升為 confirmed（同一交易內清掉 exclusion、記下案例）。擇一即可，建議用前者，比較簡單。
2. 在 crates/baton-harness/tests/exploration_to_reproduction.rs 補一個情境測試：排入重現 → 嘗試排除（expected_design 也要測）→ 應被拒絕而且沒有建立裁定請求 → supervise → 疑點成為 confirmed。
3. 把這項決策追加到 ai-decisions.md，並標記重要度。
```
