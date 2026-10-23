```
VERDICT: PASS
CRITERIA:
- [PASS] Controller Run 非終態期間，Scheduler 不把該 Profile 分給新的 Worker；已在執行的 Worker 繼續執行；Run 結束後該 Profile 恢復為候選 — 候選條件加在 crates/baton-domain/src/scheduler.rs:392-400：`controller_accounts` 內的帳號會被略過，等待原因是 `controller_account`。帳號清單來自 crates/baton-store/src/controller_cli.rs:966，只查 kind='cli' 且狀態為 starting/running 的 Run，再接到所有 Capacity 建構處：scheduling.rs:107/139/239 與 handoffs.rs:614（交接承接者也排除）。Harness 測試 `while_the_controller_cli_is_open_its_account_gets_no_new_workers_until_it_ends` 驗了四件事：新 Task 排隊且原因是 controller_account、既有 Worker 沒被 stop 且仍是 running、end 之後排隊的 Task 派到這個帳號、既有 Worker 照常 succeeded。`another_account_takes_the_work_while_the_controller_account_is_excluded` 與 domain scheduler/tests.rs:455-486 的 3 個單元測試也都通過。
- [PASS] Controller 頁簽與 Profile 選單顯示額度與執行中 Worker 數；達到 draining 門檻時顯示提示 — web/src/ControllerPage.tsx：`quotaSummary`（「5h 剩 N%｜7d 剩 N%｜執行中 Worker N」）同時用在開啟用與換帳號用的下拉選單（`optionLabel`），開著時頁簽也顯示 Controller 帳號的額度（aria-label「Controller 帳號額度」）。`quota_gate.state === "draining"` 時顯示 role=status 提示。資料取自既有的 GET /scheduling，Controller 開著時每 2 秒重讀。元件測試涵蓋選單額度與「未知」、讀取失敗、開著時輪詢更新、draining 提示（ControllerPage.test.tsx:407）。
- [PASS] 同一 Workspace 以同一 Profile 重開時啟動規格含 `--continue`；換 Profile 時不含 — 規則在 domain controller_cli.rs `continues_session`：最近一次跑過的 Run 用同一個 Profile 才接續。開窗前再透過 `continue_capability` 確認原生 Session 檔存在（claude_adapter.rs:602-625，supervisor/controller_cli.rs:715、770-779）。`reopening_with_the_same_profile_continues_the_native_session` 用正式的 ClaudeCodeAdapter 驗證：同帳號且有 Session 時帶 `--continue`；換 Profile、改開其他 Workspace、上次沒有任何對話時都不帶。換帳號重開開出的 Run 一律 continue_session=false（`open_switched`）。
- [PASS] 「換帳號重開」先結束目前 Controller Run 再開新的，兩者在時間線上可追查 — 新增 POST /controller-cli/switch（api controller_cli.rs:40-49）。Store 的 `end_cli` 在結束舊 Run 的同一個交易中開新 Run，新 Run 的 `switched_from_run_id` 存在既有欄位 handoff_from_run_id，有外鍵。`switching_accounts_ends_the_current_cli_then_opens_a_new_session_with_the_new_profile` 斷言：舊宿主被 stop、舊憑證回 401、新 Run 用新帳號的 CLAUDE_CONFIG_DIR 且不帶 `--continue`，時間線最後 5 筆依序是 switch_requested（含 from/to）→ ended(end_code=switched) → opened(switched_from_run_id 指向舊 Run) → launch_started → running。另有測試涵蓋：宿主先自行結束、拒絕同帳號／不可用帳號／已在結束中、重新綁定後換帳號、事件游標接續。
- [PASS] 以 Runtime harness 撰寫情境測試涵蓋上述行為並通過；Controller 頁簽元件測試涵蓋額度與換帳號 — crates/baton-harness/tests/controller_cli_accounts.rs 的 8 個測試全部 ok（log 中 "8 passed; 0 failed"）。ControllerPage.test.tsx 新增 9 個 it，涵蓋額度、draining 提示、換帳號選單、呼叫 switch、被拒時的警示、Session 說明、switched 的結束原因。baton-client 也新增 switch 的測試。
- [PASS] AI 決策已追加至 ai-decisions.md — 追加了 AI-41～AI-45，沒有刪改既有條目（diff 中沒有任何刪除行）。格式小瑕疵：AI-44、AI-45 少了「- 審閱：未審」，條目之間也少了空行。close.sh:16 會跑 normalize_decisions.py 補上，所以不擋結案。
TESTS:
- `git merge-base --is-ancestor cli/v1 HEAD` → ANCESTOR_OK（cli/v1 是 HEAD 的祖先）
- `pnpm test`（scripts/test.mjs：clippy --workspace --all-targets -D warnings；cargo test --workspace --exclude baton-desktop；pnpm --filter web test；UI E2E）→ EXIT=0，Clippy 通過、Rust 通過（沒有任何 failed）、前端 30 個 Test Files／675 個 Tests 全部通過、UI E2E 通過（3 passed）
- `pnpm build`（release 版 baton-runtime/baton-guardian 加 tauri build --no-bundle）→ EXIT=0，產出 target/release/baton-desktop.exe
- 跑完後 `git status --short` 是空的（沒有改動追蹤中的檔案）
FIXES_NEEDED: none
（非阻擋的觀察：①AI-44、AI-45 的格式缺漏由 close.sh 正規化處理。②已要求換帳號、但宿主還沒結束時 Runtime 當掉，若重啟後查到宿主已不在，Run 會走 interrupted 路徑，換帳號要求不會開出新 Run，使用者得手動重開。這是極端時序，spec 沒有規定，可以列為後續改進。）
```
exit: 0

