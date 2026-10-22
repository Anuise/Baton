```
VERDICT: PASS
CRITERIA:
- [PASS] 前置：rebuild/v1 是 HEAD 的祖先 — 執行 `git merge-base --is-ancestor rebuild/v1 HEAD` 結束碼為 0（輸出 ANCESTOR_OK）；rebuild/v1 = 554da20，HEAD 6f1ad82 是合併它的 commit。
- [PASS] 只有文字成功或程序正常退出時 Task 不會變成 completed — Run succeeded 只把 Task 推進到 verifying（crates/baton-domain/src/run.rs 的 `finish(RunStatus::Succeeded, …, TaskStatus::Verifying)`，整合分支原本就是這樣）。要進入 completed 只有一條路：`verification::decide` 中結果為 Passed（crates/baton-domain/src/verification.rs，decide 結尾的 `VerificationOutcome::Passed => TaskStatus::Completed`），而 decide 只能從兩處呼叫：RecordChecks（Supervisor 執行完檢查）與 AcceptManually（桌面 API）。情境測試 crates/baton-harness/tests/acceptance.rs:144 `a_worker_claiming_success_and_exiting_normally_does_not_complete_the_task` 讓 Worker 呼叫 report_progress 宣稱「全部完成」並正常退出，再跑 3 次 supervise，Task 仍是 verifying、沒有任何紀錄、沒有執行任何命令。測試通過。
- [PASS] 過期 Run 的成果回報不能推進 Task — 接受交易的校驗一（verification.rs decide：`subject_run` 必須是世代最大且 succeeded，且 run_id 與 generation 都相符，否則回 `acceptance_run_stale`）。acceptance.rs:353 `reports_from_a_superseded_run_cannot_advance_the_task`：世代 1 被接替後，它的 register_artifact 回 409 run_report_stale；它的成功結束事件沒有讓 Task 離開 running；manifest 是空的；以世代 1 為依據的接受回 409 acceptance_run_stale；以世代 2 為依據的接受成功。另外 :442 測了契約版本與 manifest 過期的情況，:478 測了檢查與手動接受的競態。全部通過。
- [PASS] 確定性檢查失敗時 Task 回到可修正狀態並保留證據 — decide 中結果為 Failed 時走 `TaskStatus::Ready, "verification_failed"`；證據寫進追加式資料表 task_verification（crates/baton-store/src/lib.rs 的 migration 16，有觸發器禁止更新與刪除；寫入在 store/verification.rs insert_record）。acceptance.rs:218：結束碼 101 → Task 變回 ready，紀錄的 detail 含 101 與輸出結尾，工作面板看得到這筆紀錄；修正後世代 2 通過，兩筆紀錄 [(1,failed),(2,passed)] 都保留。:255（成果檔案被竄改）、:276（程式無法啟動、檔案不存在）也都判為未通過。
- [PASS] 使用者手動接受需經桌面憑證，並記錄於事件 — crates/baton-api/src/verification.rs 的 accept 從 guard 驗證過的 `CredentialRecord` 取得接受者（token_hash 前 16 字元），請求本身不能指定；路由在 /api/v1/ 之下，只接受桌面角色。acceptance.rs:395：Worker 自己的 Agent 憑證、另一個 Agent 憑證、沒帶憑證，三種都得到 403/401，Task 不動；用桌面憑證接受後產生一筆 `task_verification_recorded` 事件，含 method=manual、accepted_by 與 run_id，而且 accepted_by 不是憑證的子字串。對已完成的 Task 再接受一次回 409 task_not_verifying。
- [PASS] 以主要 seam（Runtime harness，D-04）撰寫的情境測試涵蓋本票並通過；AI 決策已追加 — crates/baton-harness/tests/acceptance.rs 共 12 個情境，`running 12 tests … test result: ok. 12 passed`。另有 Domain 單元測試（verification/tests.rs）、真實程序測試 crates/baton-runtime/tests/command_runner.rs、用戶端測試 baton-client/tests/verification_client.rs。ai-decisions.md 已追加 AI-148～AI-158，每條都標了重要度（高／中／低）。
TESTS:
- `pnpm install --frozen-lockfile` → 完成
- `pnpm build`（cargo build --release -p baton-runtime 加上 tauri build --no-bundle，其中包含 web 的 tsc --noEmit 與 vite build）→ EXIT=0，產出 target/release/baton-desktop.exe
- `pnpm test`（cargo test --workspace --exclude baton-desktop 加上 pnpm --filter web test）→ EXIT=0。Rust 的每個 test binary 都是 ok、0 failed，只有一個 binary 有 3 個 ignored，其中 baton-domain 287 passed、acceptance 12 passed；前端 Test Files 14 passed、Tests 243 passed。結尾為「Rust：通過／前端：通過」。
- 執行完後 `git status --porcelain` 為空，沒有改到任何追蹤中的檔案。
FIXES_NEEDED: none
```

以下幾點不影響判定，但值得注意：
- **自動檢查進行中可以手動接受**：使用者在檢查跑的時候手動接受，即使檢查本來會失敗，Task 仍會完成。這是刻意的設計（AI-152），票面寫的「確定性檢查或手動接受」也允許這樣做。
- **已記錄的已知限制**：驗證命令沒有放進 Job Object，逾時後由它再開的子孫程序可能殘留；檔案檢查不處理指向工作目錄外的符號連結或 junction。兩者都寫在 docs/development.md 的「已知限制」，屬後續工作。
- **命令輸出的證據**：證據裡的輸出是 stdout 後面接 stderr，不是兩者依時間交錯的結果，所以不一定照實際發生順序。
