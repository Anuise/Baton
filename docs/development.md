# 開發者指南

Baton（暫名，見 spec D-01）是 Windows 桌面應用程式：Rust 寫的常駐 Runtime、React + TypeScript + Vite 的 Web UI、Tauri 桌面殼。舊版 skill（`skills/`、README）在重建期間不動，已用 git tag `archive/legacy-skill` 封存。

## 目錄

| 路徑 | 內容 |
| --- | --- |
| `crates/baton-domain` | Domain 核心（純邏輯、無 I/O）：產品名稱常數、時間戳、Runtime 設定解析、平台支援狀態判定與支援期限表（`platform.rs`）、Workspace 狀態機（`workspace.rs`：命令 → 新狀態 + 事件）、account_profile 狀態機與可派工判定（`profile.rs`）、首次啟動精靈的工具檢查與背景行為告知（`setup.rs`）、資料目錄位置判定（`storage_location.rs`） |
| `crates/baton-ports` | 所有外部副作用的 Port 介面：Clock、ProcessControl、AgentAdapter、QuotaObserver、WorkspaceFs、PlatformInfo、SecretProtector、ClaudeEnvironment |
| `crates/baton-store` | SQLite 存取：單一寫入佇列、PRAGMA 讀回驗證、`schema_migrations`、Workspace、account_profile 與事件表 |
| `crates/baton-api` | Axum 路由（`/api/v1/health`、`/api/v1/platform`、`/api/v1/workspaces`、`/api/v1/profiles`、`/api/v1/setup`、`/api/v1/events`）與入口防護 `guard`（憑證、Host、Origin、CORS） |
| `crates/baton-credential` | 桌面憑證檔（`auth/desktop-credential.bin`）的讀寫與 DPAPI 保護 |
| `crates/baton-runtime` | Runtime 組裝（`Runtime::start`）、單實例鎖、`baton-runtime.exe`、正式的 Claude Code／Git 環境探測（`claude_cli.rs`） |
| `crates/baton-harness` | 測試 harness 與全部 Port 的 fake（主要測試 seam，spec D-04） |
| `crates/baton-client` | 桌面殼用的 `RuntimeClient`（自動讀取憑證）與「連回既有 Runtime 或啟動新的」邏輯 |
| `src-tauri` | Tauri 桌面殼與安裝設定（`tauri.conf.json`） |
| `web` | React + TypeScript + Vite 的 Web UI |
| `scripts` | 建置、測試、冒煙測試腳本 |

## 環境需求

- Windows 11 x64
- Rust stable（`rustup`）與 **含「使用 C++ 的桌面開發」工作負載的 Visual Studio / Build Tools**
- Node.js 22 以上、pnpm 9
- WebView2 Runtime（Windows 11 內建）

安裝前端相依：

```powershell
pnpm install
```

### 關於 `scripts/with-msvc.ps1`

rustc 預設會挑最新的 Visual Studio；若那一版沒有安裝 C++ 桌面工作負載，連結時會出現 `LNK1104: cannot open file 'msvcrt.lib'`。`scripts/with-msvc.ps1` 以 `vswhere` 找到有 C++ x64 工具元件的安裝並載入它的環境後再執行指令：

```powershell
powershell -NoProfile -File scripts/with-msvc.ps1 cargo test -p baton-store
```

`pnpm build`、`pnpm test` 已經透過它執行。如果你是在「開發人員命令提示字元」或工具鏈本來就正常，直接用 `cargo` 即可。

## 建置

一條指令建置 Runtime、Web UI 與 Tauri 桌面 App：

```powershell
pnpm build
```

產物在 `target/release/`：`baton-runtime.exe`（Runtime）與 `baton-desktop.exe`（桌面 App，內嵌 `web/dist`）。兩個檔案必須放在同一個資料夾，App 啟動時會到自己旁邊找 Runtime。

要產生 NSIS 單使用者安裝程式（免系統管理員；首次需下載 NSIS）：

```powershell
pnpm build --bundle    # 輸出 target/release/bundle/nsis/*-setup.exe
```

開發時：

```powershell
pnpm --filter web dev                    # 只開 Web UI；/api 由 Vite 代理到 127.0.0.1:47821
cargo run -p baton-runtime               # 啟動 Runtime（預設資料目錄 %LOCALAPPDATA%\Baton）
pnpm tauri dev                           # 開桌面殼（會自動啟動 Web UI 開發伺服器）
```

Runtime 的設定環境變數：

| 變數 | 用途 | 預設 |
| --- | --- | --- |
| `BATON_DATA_DIR` | 資料目錄（控制資料庫放在其中） | `%LOCALAPPDATA%\Baton` |
| `BATON_PORT` | 監聽的 loopback 埠 | `47821` |

### 控制資料庫與 Workspace

控制資料庫是 `<資料目錄>\control.sqlite`（SQLite，WAL）。

- **單一寫入佇列**：Store 有一條專用執行緒擁有唯一的寫入連線，所有寫入都排進佇列，各自在一個短交易（`BEGIN IMMEDIATE`）中執行；唯讀查詢走另一條 `query_only` 連線。對外沒有「直接寫 SQL」的入口，只有「套用 Domain 結果」的方法，例如 `apply_workspace_command`：同一個交易中載入目前狀態 → Domain `decide` 判定 → 寫入新狀態與事件。
- **PRAGMA**：`journal_mode=WAL`、`synchronous=FULL`、`foreign_keys=ON`、`busy_timeout=5000`（issue #3 §11.1）；每條連線開啟後讀回驗證，任何一項沒生效就拒絕啟動。
- **schema_migrations**：記錄版本、名稱與套用時間（UTC）。資料庫的版本比程式新時，Runtime 拒絕啟動，且每次寫入前都重新檢查，所以不會用舊版修改新版資料。
- **事件表 `event`**：追加式（觸發器禁止更新與刪除），含聚合識別、聚合版本與 UTC 毫秒時間。
- **時間**：資料庫與 API 一律是 UTC 毫秒；Web UI 以系統時區顯示（`web/src/time.ts`）。
- **資料位置**：資料目錄在網路共享（UNC，含對應成網路磁碟機者）或雲端同步資料夾（OneDrive、Dropbox、Google Drive、iCloud Drive、Box、Nextcloud，以及環境變數 `OneDrive*` 指向的位置）時，Runtime 與桌面殼都會拒絕並說明原因。

Workspace API（桌面憑證才能使用）：`GET /api/v1/workspaces`（含已封存）、`POST /api/v1/workspaces`（`{ "path", "name"? }`，目錄必須存在，可以不是 Git repo）、`POST /api/v1/workspaces/{id}/archive`。錯誤一律是 `{ "error": <穩定代碼>, "message": <說明> }`。

### 帳號 Profile 與首次啟動精靈

每個 account_profile 有自己的 `CLAUDE_CONFIG_DIR`：`<資料目錄>\profiles\<profile id>`（由 Runtime 建立，識別是隨機值；任兩個 Profile 的目錄不相同也不互相包含，已封存的 Profile 目錄不會被重用）。資料庫只保存核對結果（登入與否、認證來源、帳號身分、核對時間），登入資料（token）只存在該目錄中、由 Claude Code 保管，不進資料庫、日誌、事件、API 回應或備份。最多同時 8 個使用中的 Profile（spec D-32）；封存會釋出名額但不刪除目錄。

- `GET /api/v1/profiles`：全部 Profile（含已封存）、`limit`，每個 Profile 帶 `dispatchable` 與 `blockers`（`{code, message}`）。**Scheduler 之後只能派給 `dispatchable` 的 Profile。**
- `POST /api/v1/profiles`（`{ name, expected_account? }`）：第 9 個回 409 `profile_limit_reached` 並說明上限；被拒絕時不會留下目錄。
- `POST /api/v1/profiles/{id}/verify`：以該目錄執行 `claude auth status` 核對。實際認證來源不是該 Profile 的原生登入（`ANTHROPIC_API_KEY`、`apiKeyHelper`、`CLAUDE_CODE_OAUTH_TOKEN`、第三方供應商等）、帳號與預期身分不符、設定目錄不符、同一帳號已被較早的 Profile 使用，都會標示並阻擋該 Profile 被派工。沒有預期帳號時，第一次在原生登入核對到的帳號會固定為預期身分。Claude Code 沒有回應時回 502 `profile_check_failed`，不改寫既有結果。
- `POST /api/v1/profiles/{id}/login-terminal`：開啟終端機，以該 Profile 的 `CLAUDE_CONFIG_DIR` 執行 `claude auth login`（`cmd /c start … cmd /k claude auth login`）。登入完成後由使用者按「重新核對」。
- `POST /api/v1/profiles/{id}/archive`。
- `GET /api/v1/setup`：精靈狀態（Claude Code 是否在 PATH 與版本、Git、使用中 Profile 的登入狀態、`ready`、背景行為告知）；`POST /api/v1/setup/recheck`（「重新檢查」，另外重新核對每個使用中的 Profile）；`POST /api/v1/setup/background-notice/acknowledge`。
- 缺項只回傳官方指令供顯示（`guidance`），Runtime 不下載、不執行任何安裝腳本；Port 上也沒有安裝動作。最低 Claude Code 版本暫定 2.1.0（`MIN_CLAUDE_CODE_VERSION`，Adapter 驗證時固定）。

核對時沿用 Runtime 目前的環境變數，只覆寫 `CLAUDE_CONFIG_DIR`，所以環境變數造成的覆蓋會如實顯示。正式實作（`ClaudeCli`）在本機的手動驗證：

```powershell
# 需要本機已安裝 Claude Code 與 Git
powershell -NoProfile -File scripts/with-msvc.ps1 cargo test -p baton-runtime --lib -- --ignored live_
# 指定一個已登入的設定目錄，確認會得到原生登入與帳號
$env:BATON_LIVE_PROFILE_DIR = "C:\Users\me\.claude-Work-1"
powershell -NoProfile -File scripts/with-msvc.ps1 cargo test -p baton-runtime --lib -- --ignored live_auth_status_of_a_logged
```

登入終端機會開啟真正的視窗，自動化測試不涵蓋；需要在 Windows 上手動按「在終端機登入」確認視窗出現、`CLAUDE_CONFIG_DIR` 是該 Profile 的目錄、登入後「重新核對」顯示原生登入與帳號（Validation Required）。

### 本機 API 的存取防護

Runtime 同時監聽 IPv4 與 IPv6 loopback（`127.0.0.1:<port>`、`[::1]:<port>`），拒絕非 loopback 的監聽位址。所有端點（含 `/api/v1/events` 串流與未知路徑）都要求：

- `Host` 是 loopback IP 字面值加上 Runtime 的埠；
- 若帶 `Origin`，必須是 Tauri App 的 origin（`http(s)://tauri.localhost`、`tauri://localhost`）；`OPTIONS`（CORS 預檢）一律拒絕、不回任何 `Access-Control-*` 標頭；
- 固定標頭 `X-Baton-Client: baton-desktop`；
- `Authorization: Bearer <桌面憑證>`。

桌面憑證是 256 位元隨機 token，Runtime **每次啟動都重新產生**，資料庫只存 SHA-256 雜湊，明文以 DPAPI（目前使用者）保護後寫到 `<資料目錄>\auth\desktop-credential.bin`。桌面 App 每次呼叫都重新讀這個檔案，所以不需要人工複製密鑰，Runtime 重啟後也會自動換用新憑證；被拒絕時 App 顯示 Runtime 回報的原因代碼（例如 `credential_invalid`），不會改用免驗證的方式。日誌在 `<資料目錄>\logs\runtime.log`，只記方法、路由樣板、狀態碼與拒絕原因，不含 URL 實際內容或憑證。

> 用 `pnpm --filter web dev` 直接以瀏覽器開 Web UI 時，請求不會帶憑證與固定標頭，會被 Runtime 拒絕（狀態列顯示「Runtime 回應 403」）。這是刻意的：Web UI 只能在桌面 App 內操作（`pnpm tauri dev`）。

單實例以「資料目錄」為單位（具名 mutex `Local\Baton.Runtime.<資料目錄雜湊>`）：同一個資料目錄只會有一個 Runtime；不同資料目錄可以並存，方便測試。

## 測試

一條指令跑完 Rust 與前端測試：

```powershell
pnpm test
```

等同於：

```powershell
powershell -NoProfile -File scripts/with-msvc.ps1 cargo test --workspace --exclude baton-desktop
pnpm --filter web test
```

桌面殼 `baton-desktop` 編譯時需要已建置的 `web/dist`，所以不在預設的 `cargo test` 範圍內；`pnpm build` 會編譯它。

### 桌面 App 冒煙測試

`pnpm build` 之後可執行：

```powershell
node scripts/smoke-desktop.mjs
```

它會以暫存資料目錄與隨機埠啟動真正的 `baton-desktop.exe`，經由 WebView2 遠端除錯埠讀取畫面，確認：狀態列顯示「Runtime 運作中」、首次設定精靈以本機真實的 `claude`、`git` 檢查、帳號頁新增 Profile 並以真實的 `claude auth status` 核對、第 9 個 Profile 被拒絕、關閉 App 後 Runtime 仍在執行、再次開啟時連回同一個 Runtime、未帶憑證的請求被拒絕。這個測試會開啟視窗，所以不納入 `pnpm test`。

### 測試 harness（主要測試 seam）

`crates/baton-harness` 在測試程序內啟動完整 Runtime（真實 Domain、真實 Store 與暫存目錄中的 SQLite、真實 HTTP API），只替換 Port：

```rust
let mut harness = Harness::start().await;               // 全部 Port 都是 fake
harness.clock.advance_millis(60_000);                   // fake 時鐘：不使用真實 sleep
harness.process_control.spawn_count();                  // 斷言 fake 被呼叫的方式
let health: HealthResponse = harness.get_json(HEALTH_PATH).await;   // 真實 HTTP，自動帶桌面憑證
let raw = harness.request(Method::GET, HEALTH_PATH).send().await;   // 不帶任何驗證標頭，用來測拒絕情境

harness.restart().await;                                // 丟棄 Runtime、保留資料目錄後重建
let (status, body) = harness.send_json(Method::POST, "/api/v1/workspaces", Some(json!({ "path": dir }))).await;  // 帶憑證，不論成敗都回傳狀態碼與 JSON
```

- `Harness::start()` / `Harness::builder().clock_at(..).start()`：啟動；每個 Harness 使用獨立的暫存資料目錄與作業系統配置的埠。
- `harness.desktop_credential()`：與 App 相同，從 DPAPI 保護檔（測試中是 fake 保護器）讀出目前的桌面憑證；`harness.issue_credential(role, ttl)` 簽發額外憑證（Agent 角色或會過期的憑證）；`harness.log_text()` 讀日誌。
- `harness.send_json(method, path, body)`：帶桌面憑證發出請求並回傳 `(狀態碼, JSON)`，用來斷言拒絕情境的錯誤代碼；`harness.db()`：以唯讀方式開啟控制資料庫，只用來確認「事件有追加、時間以 UTC 存放」這類必須看落地內容的事實。
- `Harness::builder().prepare_data_dir(..).try_start()`：Runtime 第一次啟動前先整理資料目錄（例如放入較新版本的資料庫），並回傳啟動錯誤而不是 panic。
- `harness.restart()`：模擬重啟。舊的 Runtime 被丟棄（原位址不再回應），資料目錄與全部 fake 都保留。
- `harness.clock`、`process_control`、`agent_adapter`、`quota_observer`、`workspace_fs`、`platform_info`、`secret_protector`、`claude_env`：具體的 fake，可腳本化回應並讀取呼叫紀錄。`claude_env` 可設定 Claude Code／Git 是否存在與版本（`set_claude`、`set_git`、`fail_tool_probes`）、在某個設定目錄完成原生登入（`log_in`，會像真實的 Claude Code 一樣把登入資料寫進該目錄）、模擬環境變數覆蓋（`set_env_override`）、讓開啟登入終端機時順便登入（`script_terminal_login`），並記錄開啟過的終端機（`login_terminals`）。
- 示範測試：`crates/baton-harness/tests/health_demo.rs`；本機 API 防護情境：`crates/baton-harness/tests/api_auth.rs`；Workspace 與控制資料庫情境：`crates/baton-harness/tests/workspaces.rs`；帳號 Profile 與首次啟動精靈情境：`crates/baton-harness/tests/profiles_and_first_run.rs`。

後續的票撰寫情境測試時，一律透過這個 harness 操作真實 API；不要直接呼叫內部函式或斷言資料表欄位。

### CI

`.github/workflows/ci.yml` 在 `windows-latest` 上執行 `pnpm test` 與 `pnpm build`。

## 產品名稱

產品名稱集中在 `crates/baton-domain/src/product.rs`（`PRODUCT_NAME`、`DATA_DIR_NAME`）與安裝設定 `src-tauri/tauri.conf.json`（`productName`、視窗 `title`）。`crates/baton-domain/tests/product_name.rs` 會檢查兩處一致；Web UI 畫面上的名稱取自 Runtime 回報，不寫死。改名時只需修改這幾處（以及既有使用者的資料目錄）。

## 平台辨識與支援期限表

- PlatformInfo Port 只回報事實（OS build、功能版本、版本別、原生架構、程序架構）；支援狀態由 `baton-domain::platform::assess` 依事實與 Clock 判定，所以到期判斷可用 fake Clock 測試。
- 正式實作在 `crates/baton-runtime/src/windows_platform.rs`：Windows 11 以 build ≥ 22000 判定（登錄的 `ProductName` 在 Windows 11 上仍可能寫 Windows 10），原生架構取自 `IsWow64Process2`。
- 支援期限表（`SUPPORT_TABLE`）的來源與查閱日期寫在同一檔案；新增驗證通過的功能版本、或查到官方日期時，一併更新 `SUPPORT_TABLE_CHECKED_ON`。
