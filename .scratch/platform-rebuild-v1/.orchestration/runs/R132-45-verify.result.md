```
VERDICT: PASS
CRITERIA:
- [PASS] 前置條件：rebuild/v1 是 HEAD 的祖先 — 驗證一開始時 `git merge-base --is-ancestor rebuild/v1 HEAD` 輸出 ANCESTOR_OK（當時 rebuild/v1 = 5c05582，已併入 6cf1860）。驗證途中 rebuild/v1 前進到 9105144（「chore(scratch): 記錄 R130，準備 R133」），這個 commit 只改了 .orchestration/runs.md 和 R133-44-verify.prompt.md，所以重新檢查時已不是祖先。依指示只標註，不因此判 FAIL。
- [PASS] Worker 宣稱成功但實際副作用不符時，驗證失敗 — crates/baton-harness/tests/verification_profile_cli_api.rs:293-373：Worker 回報完成、程序也以 success 結束，但真實介面讀不回資料，這時 Bundle 是 verdict=product_failure、reason_code=assertion_failed（:324），read-back 步驟的 json_equals 斷言實際值為「不存在」（:328-330）。拿這份 Bundle 當 Stage 證據會得到 incomplete/not_passed（:339-342）。Worker 呼叫 run_verification 會收到 403 controller_only（:312-313）。用正式 Port 對真實 axum 服務也驗過：verification_real_adapter.rs:124-126，服務回 201 但沒有保存資料 → product_failure。判定的邏輯在 verification_profile.rs:918-982 的 judge_step，只看觀察到的結果和程式斷言，不讀 Worker 的敘述。
- [PASS] 證據不足與環境阻礙不算通過 — verification_profile_cli_api.rs:378-454 逐一驗了這些情況：doctor 找不到程式 → environment_blocked/operation_not_started，服務不啟動、旅程不執行；/health 一直回 503 → environment_blocked/not_ready，試 3 次，fake 時鐘前進 1000ms；401 → auth_blocked/http_unauthorized，後續步驟記 not_executed；逾時 → inconclusive；人工步驟 → automation_unsupported；預算用完 → budget_exhausted。七種判定裡沒有一份是 passed 或 product_failure（:450-453）。domain 層 aggregate 在 verification_profile.rs:1042-1056：沒有旅程時判 inconclusive，前置步驟受阻時優先於旅程結果。Stage 證據的查驗在 baton-store/src/verification_profiles.rs:141-161，必須 verdict==Passed、Profile 是目前版本、需求版本沒變。
- [PASS] 新 Profile 至少實際跑過一條代表旅程，並標示未覆蓋範圍 — verification_profile_cli_api.rs:459-521 驗了：沒有代表旅程或步驟沒有斷言時，發布回 422；剛發布時 proven_by 是 null；版本還沒證明前，每次要求都會自動加入代表旅程（:485）；環境受阻時仍然沒有證明（:489）；代表旅程通過後 proven_by 才設上（:496）；uncovered 會列出 Feature Map 中沒有旅程的功能、明確宣告的範圍，以及這次沒跑到的旅程；發布新版本後要重新證明（:518）。另外 verification_real_adapter.rs:96-142 用正式的 LocalVerificationDriver 實際送出 HTTP、執行 cmd、寫入並移除狀態目錄，產品修好後 passed，proven_by 設定，uncovered=「匯出 CSV」。
- [PASS] 以 Runtime harness（spec D-04）寫的情境測試涵蓋本票行為並通過；AI 決策已追加 — 情境測試 verification_profile_cli_api.rs 有 6 個測試，全部通過，另外涵蓋 Bundle 綁定 build/revision/需求/Profile 版本（:526-557）、清理只移除這次的狀態並保留證據（:562-618）、重啟後的佇列要求仍會執行（:623-641）。verification_real_adapter.rs 也通過。ai-decisions.md 新增了 AI-391～AI-398，每條都有重要度（高 1、中 4、低 3）。
- [PASS] 票面其他內容（spec D-29：CLI／API Adapter、操作與判定分離、程式斷言優先、七種 verdict 加 reason_code、Evidence Bundle 綁定、清理範圍）— 操作和判定分開：verification_adapter.rs 只負責執行操作，判定全部交給 baton_domain::verification_profile。七種 Verdict 定義在 verification_profile.rs:735。Bundle 欄位在 :1076-1103。清理在 verification_adapter.rs:264-284：teardown、停止服務、移除 `<data>\verification\<request_id>`，不論結果如何都會執行。
TESTS:
- `pnpm test` → 結束碼 0。Rust 共 1533 passed、0 failed、8 ignored；ignored 的都是既有、需要已登入 Claude Code 的 live 測試，不在本票範圍。前端 vitest 28 個檔案、593 個測試全部通過。
- `pnpm build` → 結束碼 0，產出 target/release/baton-desktop.exe 等檔案。有 1 個 warning（unused import `StoreError`，在 crates/baton-api/src/shared_settings.rs:20），這個檔案不在本票 diff 內，是原本就有的。
- 執行前後 `git status --short` 都是空的，沒有修改任何追蹤中的檔案。
FIXES_NEEDED: none
```

以下是不影響判定的觀察，留給後續的票參考：

1. **Bundle 沒有比對 Workflow 綁定的 Task。** Stage 查驗 `verification` 證據時（`verification_evidence_status`），沒有確認 Bundle 的 `task_id` 是否就是 Workflow 綁定的那個 Task。因此 Controller 可以拿 Workspace 層級、或別的 Task 的 passed Bundle 當證據。既有的 `task_accepted` 證據也沒有做這個比對，所以和票 43 的設計是一致的。
2. **Bundle 沒有比對目前的 revision 或 build。** 驗證通過後如果再改程式碼，舊的 Bundle 仍然有效。這屬於票 50「Gate 失效與重驗」的範圍。
3. **長時間驗證會延後整合佇列。** 驗證要求和整合佇列共用 `verify_lock`、依序執行，所以一個很長的驗證會讓整合跟著延後。
