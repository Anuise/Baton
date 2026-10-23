VERDICT: PASS
CRITERIA:
- [PASS] 前置：`cli/v1` 是 HEAD 的祖先 — `git merge-base --is-ancestor cli/v1 HEAD` 回傳成功（輸出 `ANCESTOR_OK`），HEAD 是 `102d16b Merge branch 'cli/v1' into …`。
- [PASS] Controller CLI 啟動時以 `--append-system-prompt-file` 帶入 Run 專屬唯讀目錄裡的操作手冊，內容與版本號一致
  - 啟動前，`crates/baton-runtime/src/supervisor/controller_cli.rs:320-339` 把「操作手冊＋共同指示」寫成 `run-settings\<run_id>\system-prompt.md`，寫入方式是 `write_readonly_files`。
  - `controller_cli_spec` 一律帶 `settings_dir`（`controller_cli.rs:295`），`claude_adapter.rs` 的 `controller_spawn_spec` 再把它轉成這個旗標。
  - 版本號：`PLAYBOOK_VERSION="1"`，手冊標題行是「# Baton Controller 操作手冊（版本 1）」，`playbook/tests.rs` 的 `the_playbook_title_carries_its_version` 檢查兩者一致。
  - 情境測試 `controller_playbook.rs` 的 `the_controller_cli_loads_the_versioned_playbook_from_its_read_only_run_directory` 檢查：檔案在 Run 目錄、唯讀、內容等於 `CONTROLLER_PLAYBOOK`、標題版本與 API 回報一致。另一個測試 `with_shared_settings_the_playbook_comes_first_…` 檢查有共同指示時手冊排在前面。
- [PASS] 操作手冊不含帳號名稱、額度數字、並行上限或 worktree 路徑（以測試檢查）
  - 測試 `the_playbook_has_no_account_names_quota_numbers_concurrency_limits_or_worktree_paths` 禁止 `Work-`、`.claude-`、`%`、`5h`、`7d`、`≥`、`-wt`、`../`、磁碟路徑等字串。內文出現的數字只允許「最多修正 3 次」與「結束碼 N」。
  - 我另外用 grep 對 `controller.md` 查這些字樣，沒有結果。
- [PASS] Controller 頁簽顯示操作手冊版本號
  - `ControllerCliView` 新增 `playbook_version`（`crates/baton-api/src/controller_cli.rs:48-56`），`web/src/ControllerPage.tsx:158-159` 顯示「操作手冊版本」。
  - 前端有 3 個新測試：從未開啟、有 Run、載入中三種情況。Harness 的 `the_controller_tab_reports_the_playbook_version_even_before_the_first_open` 也通過。
- [PASS] Worker Run 的系統提示包含 Worker 通則，既有的 Worker 啟動測試仍通過
  - `prepare` 寫入 `runs\<id>\system-prompt.md`，內容是 `WORKER_RULES` 加上快照裡的 `CLAUDE.md`（`claude_adapter.rs` 的 prepare），`spawn_spec` 一律帶這個檔案。快照在 `prepare` 之前就已寫好（`supervisor.rs:1141-1151`）。
  - `worker.md` 涵蓋 TDD、commit 到 worktree 分支、最多 3 個 subagent、AI 決策只能在檔尾追加。
  - 新測試 `the_worker_gets_the_worker_rules_as_its_system_prompt_even_without_shared_settings` 與 `prepare_writes_the_worker_rules_followed_by_…` 通過。原本斷言「指向快照 `CLAUDE.md`」的 4 處既有測試已改為斷言合併檔，原因記在 AI-33。其餘既有 Worker 測試全部通過。
- [PASS] Workspace 沒有 `.baton/ai-decisions.md` 時，開啟 Controller 會建立它；已存在時不覆寫
  - 實作是 `ensure_ai_decisions`（`controller_cli.rs:341-350`）加上 `create_file_if_absent`（`crates/baton-ports/src/workspace_fs.rs:147-161`，以 `create_new` 開檔）。
  - 範本與 `.scratch/platform-rebuild-v1/ai-decisions.md` 檔頭比對：重要度說明、審閱行規則、條目格式區塊與 `---` 逐字相同。只有標題、開頭說明和「spec D-01～D-33」那一行改成 Workspace 適用的寫法（記在 AI-36）。
  - 情境測試 `opening_creates_the_ai_decisions_file_…`、`an_existing_ai_decisions_file_is_never_overwritten`（開、關、再開之後內容不變）、`when_the_ai_decisions_file_cannot_be_created_…` 通過。
- [PASS] 以 Runtime harness 撰寫情境測試涵蓋上述行為並通過 — `crates/baton-harness/tests/controller_playbook.rs` 的 6 個測試全部 ok，`claude_adapter.rs` 的 harness 測試也通過。
- [PASS] AI 決策已追加至 ai-decisions.md — `.scratch/desktop-controller-cli/ai-decisions.md` 新增 AI-33～AI-37，每條都有重要度與 `- 審閱：未審`，且只有新增、沒有修改既有條目。

TESTS:
- `with-msvc.ps1 cargo clippy --workspace --all-targets -- -D warnings`：成功（EXIT=0）。
- `with-msvc.ps1 cargo test --workspace --exclude baton-desktop`：
  - 第 1 次失敗（EXIT=101）。`baton-store` 的 `lifecycle::tests::concurrent_writers_of_the_lifecycle_file_do_not_trip_over_each_other` 出現「89 次寫入失敗：存取被拒。(os error 5)」，而且 cargo 在這裡就中止，後面的 crate 沒有跑。
  - 這張票沒有改到 `baton-store`，這個測試是 8 條執行緒同時寫同一個檔案，我判斷是 Windows 環境下偶發的失敗，與本票無關。
  - 加上 `--no-fail-fast` 重跑：1886 passed、0 failed、13 ignored（EXIT=0），那個 store 測試這次也通過。
- `pnpm --filter web test`：31 個檔案、685 個測試全部通過（EXIT=0）。
- `node scripts/test-e2e.mjs`（UI E2E）：通過（EXIT=0）。
- `pnpm build`：成功，產出 `target\release\baton-desktop.exe`（EXIT=0）。
- 執行完後 `git status --short` 沒有任何變更，工作目錄仍是乾淨的。

FIXES_NEEDED: none

（以下兩點不擋合併，僅供參考）
- 開啟 Controller CLI 時，`supervisor/controller_cli.rs:91` 仍會呼叫 `agent_adapter.prepare`，所以 `runs\<controller_run_id>\` 底下也會寫出一份用不到的 Worker 通則 `system-prompt.md`。不影響行為，因為 Controller 實際載入的是 `run-settings` 裡那一份。
- 舊的 Controller 輪次（`claude -p`，票 08 會移除）在移除前也會帶 Worker 通則，AI-33 已經記錄這點。
exit: 0

