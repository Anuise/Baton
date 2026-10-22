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
| 正常完成（Prompt：只回覆 OK） | 第一個非 heartbeat 事件是帶原生 Session 識別的 `Started`；同一個 Run 只回報一個原生 Session；回覆以 `Message` 交出；沒有 `Error`；以恰好一個 `Exited { success: true }` 結尾 |
| 不存在的模型（`--model baton-contract-no-such-model`） | 同樣先回報唯一的原生 Session；結束前交出非空的 `Error`；以恰好一個 `Exited { success: false }` 結尾 |
| 啟動指令 | 同一個 `RunSpec` 永遠得到同一個 `SpawnSpec`（Runtime 重建後據此找回程序）；`command_id` 相同；Prompt 不放在指令列上 |

「啟動參數中不含任何憑證值」、`CLAUDE_CONFIG_DIR`、繼承環境變數的移除、設定快照與模型的傳入，由 `crates/baton-harness/tests/claude_adapter.rs`（真實 Adapter 接在 Runtime 上的情境測試）與 `claude_adapter.rs` 的單元測試涵蓋，每次 `cargo test` 都執行。

## 執行 live 契約

需要 Windows、已安裝並登入的 Claude Code。live 測試會真的呼叫模型（每個情境一次極短的請求；不存在的模型那一次不產生輸出 token），請使用測試帳號或額度充足的帳號，**不要用主帳號**。

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
| 2026-10-22 | Windows 11 Pro 10.0.26300 x64 | 2.1.307 | `haiku`（`claude-haiku-5-5`） | 3/3 通過 | 錄製檔即 `claude-2.1.307-*.jsonl`；init 回報 `mcp_servers: [{"name":"baton","status":"connected"}]`、`permissionMode: default`、`apiKeySource: none` |

### 2026-10-22 觀察到的真實行為

- `claude -p` 從標準輸入讀取 Prompt，`--output-format stream-json --verbose` 的第一行是 `system/init`，帶 `session_id` 與 `claude_code_version`。
- `--setting-sources project,local`、`--strict-mcp-config`、`--mcp-config <檔案>`、`--allowedTools mcp__baton` 都被接受；`--mcp-config` 指向的 shim 實際連上（`status: connected`）。
- 不存在的模型：CLI 先輸出一則 `assistant` 訊息（內容就是錯誤說明，帶 `is_api_error_message`），再輸出 `result`，其中 `is_error: true`、`subtype: success`。所以判斷失敗要看 `is_error`，不能只看 `subtype`。未登入時也是同樣的形狀（`result: "Not logged in · Please run /login"`）。
- 執行期間會出現 `rate_limit_event`；額度充足時 `status` 是 `allowed` 或 `allowed_warning`。`rejected`（真正撞到限制）的形狀尚未實測，Adapter 目前只把 `rejected` 視為限流訊號（`resetsAt` 為秒）。

### 尚未驗證（Validation Required）

- `rate_limit_event` 的 `rejected` 與撞到限制時的 `result` 形狀（屬於額度觀測與突然限流的票）。
- 原生 Session 的 resume（屬於 Session 持久化與 resume 的票）；Adapter 的 `resume_capability` 目前一律回報 `Unknown`。
- `--setting-sources project,local` 是否完全阻擋 `CLAUDE_CONFIG_DIR` 中的使用者層 `CLAUDE.md` 與熱重載（spec D-09）。Baton 的 Profile 目錄只放登入狀態，實務上沒有使用者層設定可合併，但尚未以刻意放入的設定實測。
- 非互動權限參數與 permission-prompt 工具（spec D-08）。目前 Worker 以預設權限模式執行，只預先允許 Baton 自己的 MCP 工具。
