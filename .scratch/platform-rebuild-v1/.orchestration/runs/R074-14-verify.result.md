VERDICT: PASS
CRITERIA:
- [PASS] 契約測試在 fake Adapter 上全部通過；真實 Adapter 的執行方式與結果記錄文件化 — 同一份契約在 `crates/baton-harness/tests/adapter_contract.rs` 裡跑三種對象：fake（`fake_adapter_completes`、`fake_adapter_reports_an_invalid_model`、`fake_adapter_spawn_spec`）、replay（真實 `ClaudeCodeAdapter` 讀 `tests/fixtures/claude-2.1.307-*.jsonl`）、live（`#[ignore]`）。`pnpm test` 結果是 `adapter_contract: 6 passed; 0 failed; 5 ignored`。`docs/adapter-contract.md` 寫了執行方式（環境變數、指令）、結果表（OS／Claude Code 2.1.307／模型）、實際觀察到的行為，以及還沒驗證的項目。另外我自己用 Work-3（`BATON_LIVE_PROFILE_DIR=C:\Users\User\.claude-Work-3`、`BATON_LIVE_MODEL=haiku`）跑了 live 契約，結果 `5 passed; 0 failed`，CLI 是 `2.1.307 (Claude Code)`，跟文件記錄一致。
- [PASS] 首次取得 native_session_id 後以事件條件綁定，不靜默改指其他原生 Session — 規則在 `crates/baton-domain/src/run.rs` 的 `RunCommand::BindNativeSession`：還沒綁定時寫入並發出 `run_native_session_bound`；相同 Session 不做任何事；不同 Session 只發 `run_native_session_conflict`，綁定不變。`crates/baton-runtime/src/supervisor.rs` 收到 `Started{Some}` 時套用這個規則。DB 由 migration `model_overrides_and_native_session` 補上 `run.native_session_id` 欄位，`crates/baton-store/src/runs.rs` 負責讀寫。情境測試有兩個：`the_first_native_session_is_bound_and_a_different_one_never_replaces_it`（`claude_adapter.rs:246`）確認 sess-b 不會取代 sess-a、bound 只出現 1 次、有衝突事件；`after_a_runtime_restart_the_adapter_reattaches_to_the_same_run_without_a_second_launch`（`:291`）確認重建後重播同一個 Session 不算衝突，而且 spawn 只有 1 次。
- [PASS] CLI 版本不符時阻擋該 Profile 的派工並顯示原因 — `crates/baton-domain/src/claude_code.rs` 把 `MIN_SUPPORTED_VERSION` 定為 2.1.307，並提供 `check()`。`profile.rs` 的 `blockers()` 會產生 `CliVersionUnsupported` 或 `CliVersionUnknown`，訊息裡有實際版本、最低版本和「已阻擋派工」。`api/profiles.rs` 核對 Profile 時用 `probe_claude` 記下版本。情境測試有兩個：`an_unsupported_claude_code_version_blocks_the_profiles_dispatch_and_says_why`（`claude_adapter.rs:317`）確認 2.0.5 時 `dispatchable=false`、派工原因是 `cli_version_unsupported`、`spawn_count()==0`，升到 2.1.307 重新核對後才派出；`a_missing_claude_code_blocks_dispatch_as_an_unknown_version`（`:350`）確認找不到 CLI 時回 `cli_version_unknown`。UI 顯示版本由 `web/src/AccountsPage.test.tsx` 涵蓋。
- [PASS] 啟動參數中不包含任何憑證值 — `claude_adapter.rs:93-136` 的 args 只有旗標、檔案路徑和模型名稱，Prompt 經 `stdin_file` 交付。`BATON_AGENT_TOKEN` 只放在 `SpawnSpec.env`（`supervisor.rs`），而 `env` 不算進 `command_hash`，`Debug` 也只印變數名稱。情境測試 `no_credential_value_appears_in_the_launch_arguments_or_the_runs_files`（`claude_adapter.rs:194`）拿 Agent 憑證和 Profile 的 OAuth accessToken 去比對指令列、`prompt.md`、`mcp.json`，都沒有出現；`mcp.json` 只用 `${BATON_AGENT_TOKEN}` 參照。`ANTHROPIC*`、`CLAUDE*` 這些繼承變數會被移除，只保留 `CLAUDE_CODE_GIT_BASH_PATH`，由 `:159` 的測試驗證。
- [PASS] 以主要 seam（Runtime harness，D-04）撰寫的情境測試涵蓋本票行為並通過；本票的 AI 決策已追加至 ai-decisions.md — harness 新增 `HarnessBuilder::agent_adapter`，可以在真實 Runtime 上換成真實 Adapter。`crates/baton-harness/tests/claude_adapter.rs` 有 10 個情境測試全部通過，涵蓋：
  - `CLAUDE_CONFIG_DIR`、快照的 `--settings` 與 `--append-system-prompt-file`、`--setting-sources ""`、Worker 模型（D-30）
  - 模型在派工當下就固定
  - 錯誤結果會把 CLI 的原因寫進 `end_reason`
  - 快照被竄改時以 `settings_snapshot_mismatch` 阻擋啟動
  - 同一個 Run 的並行快照請求依序處理

  這也一併處理了票面兩則主控意見：一則由並行與竄改兩個快照測試涵蓋；另一則（Workspace 內的 `.claude/settings*.json`）由 `--setting-sources ""` 處理，live 測試 `live_claude_code_ignores_settings_files_inside_the_workspace` 實測通過。`ai-decisions.md` 新增 AI-196～AI-207，各條都標了重要度；跟 rebuild/v1 比只有追加、沒有刪改（`^-` 行數為 0），編號接在 rebuild/v1 最後的 AI-195 之後，沒有重複。
- [PASS] 前置：rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` 輸出 `ANCESTOR_OK`（rebuild/v1 = bd5adab）。

TESTS:
- `pnpm test`（= `cargo test --workspace --exclude baton-desktop` + `pnpm --filter web test`）：EXIT=0。Rust 的 65 個 test binary 全部 ok、0 failed，忽略的只有需要已登入 CLI 的 live 測試。前端 18 個檔案、322 個測試全部通過。
- `pnpm build`：EXIT=0，產生 `target\release\baton-desktop.exe`（release，1m57s）。
- `cargo build -p baton-mcp-shim`，接著 `cargo test -p baton-harness --test adapter_contract live_ -- --ignored --nocapture --test-threads=1`（Work-3、haiku、Claude Code 2.1.307）：5 passed; 0 failed（19.4s）。
- CI（`.github/workflows/ci.yml`）只跑 `pnpm test` 和 `pnpm build`，兩者都已通過。
- 驗證結束後 `git status --short` 是空的，沒有改動任何追蹤中的檔案。

FIXES_NEEDED: none

以下觀察不影響驗收，留給後續的票參考：
1. `spawn_spec` 會用 `is_file()` 判斷快照檔在不在，所以它實際上依賴檔案系統，並不是純函式。如果 Run 啟動之後快照檔被刪掉，Runtime 重建時對「回執未寫回」的 Run 呼叫 `find`，算出的 `command_hash` 會跟原本的指令不同。找程序是靠 command_id／Job 名稱，所以實際影響很低。
2. CLI 版本只在核對 Profile 時記錄。之後如果降版，要到下次重新核對才會被阻擋。
