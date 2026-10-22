# Adapter 契約測試：執行方式與結果記錄

Runtime harness（spec D-04 第 1 層）用 `FakeAgentAdapter` 取代真實的 Claude Code。Adapter 契約測試（第 2 層）確保這個 fake 的行為與真實 CLI 一致：同一份契約（`crates/baton-harness/tests/adapter_contract.rs`）同時跑在三種對象上。

| 對象 | 內容 | 何時執行 |
| --- | --- | --- |
| fake | `FakeAgentAdapter`，腳本化成真實 CLI 在各情境下的事件順序 | 每次 `cargo test` |
| replay | 真實 `ClaudeCodeAdapter` 讀取真實 CLI 錄下的 stream-json（`crates/baton-harness/tests/fixtures/`） | 每次 `cargo test`，不需要 CLI |
| live | 真實 `ClaudeCodeAdapter` + Windows Job Object `ProcessControl` + 已登入的 Claude Code | 手動或夜間（`#[ignore]`） |

## 契約條款

| 情境 | 條款 |
| --- | --- |
| 正常完成（Prompt：只回覆 OK） | 第一個非 heartbeat 事件是帶原生 Session 識別的 `Started`；同一個 Run 只回報一個原生 Session；回覆以 `Message` 交出；結束前以 `UsageReported` 回報用量（token 數 > 0）；沒有 `Error`；以恰好一個 `Exited { success: true }` 結尾 |
| 不存在的模型（`--model baton-contract-no-such-model`） | 同樣先回報唯一的原生 Session；結束前交出非空的 `Error`；以恰好一個 `Exited { success: false }` 結尾 |
| 啟動指令 | 同一個 `RunSpec` 永遠得到同一個 `SpawnSpec`（Runtime 重建後據此找回程序）；`command_id` 相同；Prompt 不放在指令列上 |

另外兩個只在 live 執行的驗證（對應 spec 的 Validation Required）：

| 測試 | 驗證內容 |
| --- | --- |
| `live_claude_code_ignores_settings_files_inside_the_workspace` | Workspace 內放 `.claude\settings.json` 與 `settings.local.json`（`defaultMode: acceptEdits`），`system/init` 回報的 `permissionMode` 仍是 `default`：Worker 無法藉由寫入這些檔案改變自己的權限 |
| `live_claude_code_routes_permission_prompts_to_baton_and_honours_a_deny` | 以本機的假 Agent API 扮演 Runtime：需要核准的 Bash 經 `--permission-prompt-tool` 送到 `permission_prompt`，回覆「等待裁定」（`behavior: deny` 加上 Baton 的 `status`、`decision_request_id`）後，工具沒有執行、Worker 不重試同一個操作並正常結束 |

「啟動參數中不含任何憑證值」、`CLAUDE_CONFIG_DIR`、繼承環境變數的移除、設定快照與模型的傳入，由 `crates/baton-harness/tests/claude_adapter.rs`（真實 Adapter 接在 Runtime 上的情境測試）與 `claude_adapter.rs` 的單元測試涵蓋，每次 `cargo test` 都執行。

## 執行 live 契約

需要 Windows、已安裝並登入的 Claude Code。live 測試會真的呼叫模型（每個情境一次極短的請求；不存在的模型那兩次不產生輸出 token），請使用測試帳號或額度充足的帳號，**不要用主帳號**。

```powershell
# 1. 建置 MCP shim（live 測試讓 CLI 真的連上它）
powershell -NoProfile -File scripts/with-msvc.ps1 cargo build -p baton-mcp-shim

# 2. 指定已登入的設定目錄（作為 Run 的 CLAUDE_CONFIG_DIR）與選用的模型
$env:BATON_LIVE_PROFILE_DIR = "C:\Users\me\.claude-Work-3"
$env:BATON_LIVE_MODEL = "haiku"                      # 選用；省略時用帳號預設模型
$env:BATON_LIVE_SHIM = "$PWD\target\debug\baton-mcp-shim.exe"
# 選用：把這次 CLI 的 stream-json 存成 replay 的錄製檔（更新 CLI 版本時使用）
$env:BATON_LIVE_RECORD_DIR = "$PWD\crates\baton-harness\tests\fixtures"

# 3. 執行
powershell -NoProfile -File scripts/with-msvc.ps1 cargo test -p baton-harness --test adapter_contract live_ -- --ignored --nocapture --test-threads=1
```

測試開頭會印出 `claude --version`，並要求它不低於 `baton_domain::claude_code::MIN_SUPPORTED_VERSION`。更新錄製檔後，請把檔名中的版本改成實際版本、把路徑中的使用者名稱換成範例值，並同步更新 `adapter_contract.rs` 的 `Scenario::fixture`。

## 結果記錄

每次執行 live 契約後在下表追加一列。

| 日期 | OS | Claude Code | 模型 | 結果 | 備註 |
| --- | --- | --- | --- | --- | --- |
| 2026-10-22 | Windows 11 Pro 10.0.26300 x64 | 2.1.307 | `haiku`（`claude-haiku-5-5`） | 3/3 通過 | 當時以 `--setting-sources project,local` 執行 |
| 2026-10-22 | Windows 11 Pro 10.0.26300 x64 | 2.1.307 | `haiku`（`claude-haiku-5-5`） | 5/5 通過 | 改為 `--setting-sources ""` 並加上 `--permission-prompt-tool`；錄製檔即 `claude-2.1.307-*.jsonl`；init 回報 `mcp_servers: [{"name":"baton","status":"connected"}]`、`permissionMode: default`、`apiKeySource: none` |

### 2026-10-22 觀察到的真實行為

- `claude -p` 從標準輸入讀取 Prompt，`--output-format stream-json --verbose` 的第一行是 `system/init`，帶 `session_id` 與 `claude_code_version`。
- `--setting-sources ""`、`--strict-mcp-config`、`--mcp-config <檔案>`、`--allowedTools mcp__baton`、`--permission-prompt-tool mcp__baton__permission_prompt` 都被接受；`--mcp-config` 指向的 shim 實際連上（`status: connected`）。
- `--setting-sources project,local` 會套用 Workspace 內 `.claude\settings.json` 的 `permissions.defaultMode`（實測 `acceptEdits` 生效；`permissions.allow` 因 Workspace 未經信任而被忽略並在標準錯誤提示）。`--setting-sources ""` 時兩者都不載入。
- 不存在的模型：CLI 先輸出一則 `assistant` 訊息（內容就是錯誤說明，帶 `is_api_error_message`），再輸出 `result`，其中 `is_error: true`、`subtype: success`、`api_error_status: 404`。所以判斷失敗要看 `is_error`，不能只看 `subtype`。未登入時也是同樣的形狀（`result: "Not logged in · Please run /login"`）。
- `result.usage` 有 `input_tokens`、`output_tokens`、`cache_creation_input_tokens`、`cache_read_input_tokens`；Adapter 回報前三者的和（例：2 + 4 + 6362 = 6368）。失敗的請求 `usage` 全為 0。
- `rate_limit_event` 的 `rate_limit_info` 帶 `status`（`allowed`、`allowed_warning`）、`rateLimitType`（`five_hour`、`seven_day`）、`resetsAt`（秒）、可能有 `utilization`，以及 `unifiedWindows`（每個視窗的 `utilization` 與 `resetsAt`）。
- 權限轉接：CLI 以 MCP `tools/call` 呼叫 `permission_prompt`，參數是 `tool_name`、`input`（例如 Bash 的 `command` 與 `description`）與 `tool_use_id`。回覆 `{"behavior":"deny","message":…,"status":"waiting_decision","decision_request_id":…}` 被接受（額外欄位不影響），`message` 會交給模型；被拒絕的工具沒有執行，`result.permission_denials` 列出被拒絕的呼叫。回覆 `allow` 時 `updatedInput` 是否可以省略尚未實測（Runtime 一律帶上）。

### 尚未驗證（Validation Required）

- 撞到限制時 `rate_limit_event`（`rejected`）與 `result` 的實際形狀（屬額度觀測與突然限流的票）。
- 原生 Session 的 resume（屬 Session 持久化與 resume 的票）；Adapter 的 `resume_capability` 目前一律回報 `Unknown`。
- `CLAUDE_CONFIG_DIR` 中的使用者層 `CLAUDE.md` 是否仍被載入、快照設定檔的熱重載（spec D-09）。Baton 的 Profile 目錄只放登入狀態，實務上沒有可合併的內容，但尚未以刻意放入的檔案實測。
- 權限轉接回覆 `allow` 的路徑（需要真實 Runtime 的授權判定，屬裁定中心與授權的後續驗證）。
