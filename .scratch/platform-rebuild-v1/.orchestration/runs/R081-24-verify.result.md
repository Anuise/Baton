所有檢查都做完了。第 24 張票的驗收條件全部通過，建置與全部測試也都通過。只找到兩個不影響驗收的小問題，列在最後。

```
VERDICT: PASS
CRITERIA:
- [PASS] 授權更新與准入競態測試：未准入 Run 無法取得舊授權 — `crates/baton-domain/src/run.rs` 的 `BeginLaunch` 分支（diff 中 `admit_at_launch`／`deny_admission`）會用「當下生效」的授權重新檢查。被拒時不寫 launch intent、不簽發憑證，Run 以 `queued→interrupted` 結束，Task 在同一個交易中重新排隊（`crates/baton-store/src/runs.rs` 的 `dispatch_request` INSERT）。撤權與准入都走同一個寫入佇列：`confirm_authorization` 在 `crates/baton-store/src/lib.rs:946` 用 `self.write`，`apply_run_command` 在 `crates/baton-store/src/runs.rs:54` 也用 `self.write`。`check_admission` 已依 AI-53 改走 `.write`（lib.rs:983）。Supervisor 只要看到准入後的狀態不是 `starting` 就不 spawn（supervisor.rs:522-534）。情境測試 `crates/baton-harness/tests/authorization_revocation.rs:159` 驗證：已排隊且有 Outbox 的 Run 在撤權後 spawn 次數為 0、沒有憑證、`run_admission_denied` 的 seq 晚於撤權、等待原因是 `authorization/capability_not_granted`，授權恢復後依新 revision 派出。`:200` 用 `tokio::join!` 讓撤權與准入同時送出、跑 6 輪，核對事件表 seq 的先後與 Run 實際拿到的 revision 一致。
- [PASS] 已准入 Run 沿用舊版本並在 UI 標示 — 權限提示改看 Run 准入時固定的 revision（`decision.rs` 的 `authorization_of`）；共同設定版本也在准入時固定（`settings_version`、migration `run_admission`、`for_launch(run_id, admitted)`）。API：`GET …/authorization` 的 `legacy_runs`、`GET …/tasks` 的 `authorization_revision`。UI：`web/src/AuthorizationPanel.tsx` 的 `LegacyRunsNotice` 顯示「後續執行已停用，仍有 {N} 個 Run 沿用舊授權」，並提供「暫停」（cancelRun）和「取消」（cancelTask）按鈕；`web/src/TaskBoard.tsx` 的 `RunStatusLine` 顯示「沿用舊授權（revision X，目前 Y）」。對應的情境測試：`authorization_revocation.rs:238`（撤權後仍是 running、權限提示 auto_allowed 且 revision=1、legacy_runs 內容、暫停流程）、`:287`（取消入口）、`:305`（只擴權時不列入）、`:364`（設定版本固定）。另有 Web 測試 `AuthorizationPanel.test.tsx` 與 `TaskBoard.test.tsx`。
- [PASS] 舊 Run／Session 不能成為新工作的權限捷徑 — `authorization_revocation.rs:327`：撤權後舊 Run 仍 auto_allowed，但新 Task 的 Run 是 revision=2，同一個操作會送去裁定（waiting_decision）。同一個 Task 的恢復 Run 也是 revision=2 並送裁定。舊 Run 結束後，它的憑證回 `run_report_stale`。交接 successor 還沒實作，AI-215 已註明它將來也必須經過 `BeginLaunch` 准入。
- [PASS] 主要 seam 情境測試涵蓋並通過；AI 決策已追加 — `authorization_revocation.rs` 透過 Runtime harness 的真實 HTTP API 跑 7 個測試，全部通過（`test result: ok. 7 passed`）。`ai-decisions.md` 只在檔尾追加了 AI-215～AI-221，每條都標了重要度，沒有改動既有條目。`docs/development.md` 已同步更新。
- [PASS] rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 輸出 ANCESTOR_OK（rebuild/v1 = b161486）。
TESTS:
- `pnpm test`（= `cargo test --workspace --exclude baton-desktop` + `pnpm --filter web test`）：結束碼 0。Rust 測試全部通過、0 失敗：baton_domain 378、baton_store 83、authorization_revocation 7、decisions 20、runs 21、scheduling 32、tasks 54 等，其餘都是 ok，ignored 的只有需要真實環境的 live／adapter 契約測試。前端 Vitest 18 個檔案、350 個測試全部通過。
- `pnpm build`：結束碼 0，產出 `target/release/baton-desktop.exe`。有 1 個警告，見下方第 1 點。
- 驗證後 `git status` 是乾淨的，沒有改動任何追蹤中的檔案。
FIXES_NEEDED: none
```

兩個不影響驗收的小問題，可以在之後順手修：

1. **多出一個編譯警告**：`crates/baton-api/src/shared_settings.rs:20` 有個沒用到的 `StoreError` import。這是本票改寫 `for_launch` 時拿掉了它唯一的用途造成的。CI 不把警告當錯誤，所以不會失敗。
2. **說明文字中間夾了一串空白**：`crates/baton-domain/src/authorization.rs:405` 縮小授權時的影響說明裡，「沿用准入時的版本，」和「撤權沒有…」之間夾了 13 個空白，看起來是折行時留下的。HTML 會把連續空白合併成一個，所以畫面上幾乎看不出來。`crates/baton-store/src/runs.rs` 的 `RUN_COLUMNS_QUALIFIED` 也有同樣的多餘空白，但那是 SQL 字串，不影響執行。
