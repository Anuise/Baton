# 開發者指南

Baton（暫名，見 spec D-01）是 Windows 桌面應用程式：Rust 寫的常駐 Runtime、React + TypeScript + Vite 的 Web UI、Tauri 桌面殼。舊版 skill（`skills/`、README）在重建期間不動，已用 git tag `archive/legacy-skill` 封存。

## 目錄

| 路徑 | 內容 |
| --- | --- |
| `crates/baton-domain` | Domain 核心（純邏輯、無 I/O）：產品名稱常數、時間戳、Runtime 設定解析、平台支援狀態判定與支援期限表（`platform.rs`）、Workspace 狀態機（`workspace.rs`：命令 → 新狀態 + 事件）、Workspace 授權範本／確認 revision／Run 准入檢查（`authorization.rs`）、Goal／Task 狀態機與六項最小派工契約（`task.rs`）、Run 狀態機／execution lease／派工 Outbox（`run.rs`）、account_profile 狀態機與可派工判定（`profile.rs`）、首次啟動精靈的工具檢查與背景行為告知（`setup.rs`）、資料目錄位置判定（`storage_location.rs`）、Agent 工具介面（`agent.rs`：環境變數名稱、MCP 設定、Worker 回報內容與長度上限）、共同設定的匯入、秘密遮蔽、差異、版本與 Run 快照渲染（`shared_settings.rs`，秘密辨識在 `shared_settings/secrets.rs`） |
| `crates/baton-ports` | 所有外部副作用的 Port 介面：Clock、ProcessControl（含 `find`）、AgentAdapter（含 `spawn_spec`）、QuotaObserver、WorkspaceFs（含 `write_readonly_files`：把一組檔案寫成唯讀目錄）、PlatformInfo、SecretProtector、ClaudeEnvironment、UserSettingsSource（唯讀讀取使用者平常使用的 Claude Code 設定，只有白名單內的三個檔案） |
| `crates/baton-store` | SQLite 存取：單一寫入佇列、PRAGMA 讀回驗證、`schema_migrations`、Workspace、account_profile、授權 revision、Goal／Task／契約版本／相依與事件表、Run／lease／Outbox 表、共同設定版本表 `shared_settings_version` 與 Run 設定快照綁定表 `run_settings_snapshot`（都是追加式） |
| `crates/baton-api` | Axum 路由（`/api/v1/health`、`/api/v1/platform`、`/api/v1/workspaces`（含授權、准入檢查、Goal、Task、Run 與 Run 設定快照）、`/api/v1/profiles`、`/api/v1/setup`、`/api/v1/shared-settings`（共同設定的預覽、採用與版本）、`/api/v1/events`）與入口防護 `guard`（憑證、角色、Host、Origin、CORS）；Agent API（`/agent/v1/tools/…`，`agent.rs`） |
| `crates/baton-credential` | 桌面憑證檔（`auth/desktop-credential.bin`）的讀寫與 DPAPI 保護 |
| `crates/baton-runtime` | Runtime 組裝（`Runtime::start`）、Supervisor（`supervisor.rs`）、Windows Job Object 的 ProcessControl（`windows_process.rs`）、單實例鎖、`baton-runtime.exe` |
| `crates/baton-mcp-shim` | MCP 工具 shim（`baton-mcp-shim.exe`）：Claude Code 以 stdio 啟動，把工具呼叫轉送到 Runtime 的 Agent API；只依賴 `baton-domain` |
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

### Workspace 授權與准入

選取目錄不等於授權（DR-20261021-03 的 I2）：Workspace 建立後沒有任何授權，使用者在授權設定畫面確認並保存後才生效。授權以「能力 + 目標資源 + 限制」表達（`baton_domain::authorization`），內建兩個可編輯的建議範本：**研究／文件**（讀取、編輯、讀取公開網頁）與**可信專案開發（P3）**（再加上建置測試與安裝專案依賴）。管理員／全域安裝、遠端寫入、部署、對外發訊、付費與正式資料操作不在任何範本內，只能由使用者在編輯時明確加入。

桌面憑證專用的端點（Agent 憑證得到 403）：

| 端點 | 用途 |
| --- | --- |
| `GET /api/v1/workspaces/{id}/authorization` | 目前生效的 revision、歷史、範本（目標資源已代入該 Workspace 目錄）、能力清單、風險說明 |
| `POST /api/v1/workspaces/{id}/authorization/summary` | 確認前的摘要（預覽，不保存）；含「可信程式以目前 Windows 使用者身分執行」的風險 |
| `PUT /api/v1/workspaces/{id}/authorization` | 確認並保存：新內容回 201 與新的 revision（含來源與時間）；內容與目前相同回 200；需要承認風險時必須帶 `acknowledged_risk: true` |
| `POST /api/v1/workspaces/{id}/admission-check` | Run 的准入檢查：`{ "required_capabilities": [...] }`；未確認回 409 `authorization_unconfirmed`，缺少能力回 409 `capability_not_granted` |

revision 存在 append-only 的 `authorization_revision` 表（觸發器禁止更新與刪除），並在同一交易追加 `authorization_confirmed` 事件（聚合版本 = revision 編號）。准入判定 `authorization::admit` 由 `admission-check` 端點與派工交易（`RunCommand::Dispatch`）共用，不另寫准入邏輯。

### Task、六項最小派工契約與工作面板

Goal 與 Task 的規則在 `baton_domain::task`（純邏輯：命令 + 目前狀態 → 新狀態 + 事件），持久化在 `baton-store` 的 `tasks.rs`，端點在 `baton-api` 的 `tasks.rs`。Task 屬於 Goal、Goal 屬於 Workspace，所有查詢與命令都以 Workspace 為界。

**狀態機**：`pending`、`ready`、`running`、`waiting_decision`、`waiting_resource`、`blocked`、`verifying`、`completed`、`cancelled`。允許的轉換集中在 `TaskStatus::allowed_next`；本票對外只有「標記 ready」（pending → ready，必須通過准入檢查）與「取消」。`completed`、`cancelled` 是終止狀態：不能再轉換、改契約或加相依。

**六項最小契約**（ADR-0011）：`objective`（目標）、`inputs`（輸入與依據）、`scope`（範圍與限制）、`deliverables`（預期成果）、`done_criteria`（完成與驗證條件）、`blocker_handling`（阻礙處理），外加 `required_capabilities`（這個 Task 需要的 Workspace 授權能力）。每個欄位是下列之一：

| 內容 | 意義 |
| --- | --- |
| `{"status":"filled","text":…,"references":[{"kind","target","version"}]}` | 已填寫；引用保存可解析的目標與適用版本 |
| `{"status":"not_applicable","reason":…}` | 不適用，必須附理由；`objective`、`done_criteria` 不可標不適用 |
| `{"status":"unknown","note"?:…}` | 明確標示未知；**不算完整**，必須補足才能 ready |
| 省略或 `null` | 未提供（另一種缺項，和「未知」分開顯示） |

引用種類：`task_contract`（同 Workspace 的 Task + 契約版本）、`goal`、`authorization`（目標是 Workspace 識別、版本是授權 revision）。儲存時不檢查引用（可先存），**准入檢查**時才判定失效：目標不存在、版本不存在、被引用的 Task 已取消。

契約每次變更（內容與目前版本不同）追加一個版本，存在追加式的 `task_contract_version` 表（觸發器禁止更新與刪除）；內容相同回 200、不新增版本。ready 的 Task 改契約會退回 pending，必須重新通過准入。

**准入檢查**（`task::admission_report`，Run 出現後派工交易也呼叫同一個函式）列出全部缺項，每項有穩定代碼：`field_missing`、`field_unknown`、`reference_invalid`、`authorization_unconfirmed`、`capability_not_granted`、`workspace_archived`。授權部分直接呼叫票 06 的 `authorization::admit`：`required_capabilities` 為空就不需要確認授權。相依是否完成**不是** ready 的條件；Scheduler 派工時才檢查，面板以 `unfinished_dependencies` 顯示等待原因。

**相依**：`task_dependency` 邊（task_id 依賴 depends_on）；自我相依 400 `dependency_self`、造成循環 409 `dependency_cycle`（訊息含完整循環路徑）；重複新增是冪等的。

桌面憑證專用的端點（Agent 憑證得到 403），都在 `/api/v1/workspaces/{id}/` 之下：

| 端點 | 用途 |
| --- | --- |
| `GET tasks` | 工作面板：全部 Goal、Task（含目前契約、相依、未完成的相依、pending 的缺項）、能力清單 |
| `POST goals` | `{ "title" }` 建立 Goal |
| `POST tasks` | `{ "goal_id", "title", "contract"? }` 建立 Task（pending、契約 v1；契約可先不完整） |
| `GET tasks/{task_id}` | Task 與契約版本歷史 |
| `PUT tasks/{task_id}/contract` | `{ "contract", "note"? }` 整份取代；新版本 201、內容相同 200 |
| `POST tasks/{task_id}/dependencies` | `{ "depends_on" }`；新增 201、已存在 200 |
| `POST tasks/{task_id}/admission-check` | 唯讀預覽：`{ "admissible", "missing": [...] }` |
| `POST tasks/{task_id}/ready` | pending → ready；未通過回 409 `task_admission_denied`，body 帶 `missing` |
| `POST tasks/{task_id}/cancel` | 取消 |

事件（追加式 `event` 表，`aggregate_type = 'task'`，版本連續遞增）：`task_created`、`task_contract_changed`、`task_dependency_added`、`task_status_changed`（含 `from`／`to`／`reason`）；Goal 是 `goal_created`。Web UI 的工作面板（`web/src/TaskBoard.tsx`）依狀態分欄顯示，pending 卡片列出缺項，並提供契約編輯器（`ContractEditor.tsx`）、版本歷史、新增相依與標記 ready／取消；從左側 Workspace 清單的「工作面板」進入。

### Supervisor、Run 與程序身分

Run 的規則在 `baton_domain::run`（純邏輯：命令 + 目前狀態 → 新狀態 + 事件 + Outbox），持久化在 `baton-store` 的 `runs.rs`，端點在 `baton-api` 的 `runs.rs`，啟動程序與核對在 `baton-runtime` 的 `supervisor.rs`。

**Run 狀態**：`queued → starting → running → succeeded／failed／interrupted／cancelled`（`RunStatus::allowed_next`）。只有 `running` 之後才有程序身分；`starting` 代表 launch intent 已持久化但回執還沒寫回。

**派工交易**（`POST …/tasks/{task_id}/dispatch`，Domain `RunCommand::Dispatch`）：在同一個交易中重新做完整准入檢查（`task::admission_report`，內部呼叫 `authorization::admit`）、確認相依都已完成、確認沒有任何尚未釋放的 lease，然後建立 Run（`queued`）、execution lease（世代 = 該 Task 的 Run 數 + 1）、派工 Outbox 項目，Task 進入 `running`。派工**不啟動程序**。

**寫入所有權**：同一個 Task 任何時刻最多一個尚未釋放的 `execution_lease`（Domain 判定 + 資料庫唯一索引 `execution_lease_one_held_per_task`；性質測試 `at_most_one_effective_writer_per_task_under_any_command_sequence`）。Run 結束或取消**確認程序停止之後**才釋放；停不掉（`StopResult::StillAlive`）就一直持有，新的 Run 派不出去。

**Supervisor**（`Supervisor::tick`，每次依序做四件事，全部冪等、只信任資料庫）：

1. 啟動派工 Outbox 的 Run：先在交易中持久化 launch intent 與 `command_id`（`starting`），才在交易外 `spawn`，再把回執（PID + 建立時間 + 指令雜湊）寫回（`running`）。Runtime 在任何一點當掉，重建後以 `command_id` 呼叫 `ProcessControl::find` 找回原程序並補寫回執，**不重複啟動**；找不到就把 Run 標為 `interrupted`（`launch_outcome_unknown`），不自動重送，由使用者重新派工。
2. 收 Adapter 事件：`Heartbeat` 續約 lease（間隔 15 秒、有效 60 秒，spec D-31）；`Exited` 只結束 Run（成功 → Task `verifying`，失敗 → Task 回到 `ready`），**不會**把 Task 標為 `completed`。
3. 取消：要求取消的 Run 先記下意圖（`cancel_requested_at`），Supervisor 停止整個程序樹，確認之後才完成取消、釋放 lease、Task 回到 `ready`；還沒啟動的 Run 立即取消。取消 Task 會在同一個交易中要求取消它進行中的 Run。
4. 核對 lease：過期只觸發核對（`inspect`）：確認 `Dead` 才中斷 Run，`Alive` 續約，`Unknown` 維持原狀。

正式執行時由背景迴圈每秒（或派工／取消時被喚醒）呼叫 `tick`；`RuntimeOptions { background_supervisor: false }`（harness 使用）不啟動背景迴圈，由測試呼叫 `harness.supervise()`。`RuntimeOptions::stop_grace` 預設 30 秒（spec D-11）。

**真實 ProcessControl**（`WindowsProcessControl`，只在 Windows 編譯）：每個 Run 一個具名 Job Object `Local\Baton-Run-{command_id}`（不設 KILL_ON_JOB_CLOSE，Runtime 崩潰不會殺掉程序）；`stop` 先在寬限期內等程序自行結束，逾時 `TerminateJobObject` 終止整棵樹。整合測試：`crates/baton-runtime/tests/process_tree.rs`（直接測 Port）、`crates/baton-runtime/tests/supervisor_process_tree.rs`（真實 Runtime + HTTP 取消）。正式執行檔的 AgentAdapter 仍是「尚未實作」（票 14），所以目前派工後 Run 會以 `launch_failed` 結束。

桌面憑證專用的端點（Agent 憑證得到 403），在 `/api/v1/workspaces/{id}/` 之下：

| 端點 | 用途 |
| --- | --- |
| `POST tasks/{task_id}/dispatch` | 派工（Task 必須是 ready）；201 回 `queued` 的 Run。409 代碼：`task_not_ready`、`task_admission_denied`（帶 `missing`）、`dependencies_unfinished`、`execution_conflict` |
| `GET runs`、`GET runs/{run_id}` | Run（含 lease、程序身分、`cancel_requested_at`、`end_reason`）；工作面板 `GET tasks` 也帶 `runs` |
| `POST runs/{run_id}/cancel` | 要求取消；已結束或重複取消是冪等的（200） |

事件（`aggregate_type = 'run'`）：`run_created`、`run_launch_started`、`run_started`、`run_lease_verified`、`run_succeeded`、`run_failed`、`run_interrupted`、`run_cancel_requested`、`run_cancelled`（heartbeat 只更新 lease，不追加事件）。

### Agent API、範圍憑證與 MCP shim

Worker 不直接碰資料庫或管理 API：它的程序啟動時帶著一組**只屬於這個 Run** 的憑證，由 MCP shim 把工具呼叫轉送到 Runtime 的 Agent API。純邏輯在 `baton_domain::agent`，端點在 `baton-api` 的 `agent.rs`，持久化在 `baton-store` 的 `agent.rs`。

**憑證怎麼交給程序**：Supervisor 在 `BeginLaunch`（launch intent 已持久化）之後、`spawn` 之前，呼叫 `CredentialIssuer::issue_agent` 為該 Run 簽發 Agent 憑證，綁定 Workspace、Run 與世代。它沒有時間上限，有效與否由「這個 Run 仍是寫入所有者」決定（所以 Runtime 重啟後，Worker 程序手上的憑證仍可用）。簽發失敗就不啟動程序。憑證只經 `SpawnSpec.env` 的環境變數 `BATON_AGENT_TOKEN` 與 `BATON_RUNTIME_URL` 交給程序，**不**進 Prompt、指令列、MCP 設定檔、日誌或資料庫明文（資料庫只存雜湊）。`SpawnSpec` 的 `Debug` 只印環境變數名稱，環境變數不納入 `command_hash`；真實 `WindowsProcessControl` 以 Unicode 環境區塊傳入（同名時以 `spec.env` 為準）。

**MCP 設定**：`baton_domain::agent::mcp_config_json(shim 路徑)` 產生的設定只含 shim 指令與 `${BATON_AGENT_TOKEN}`、`${BATON_RUNTIME_URL}` 的參照，由 Claude Code 展開；真實 AgentAdapter 在票 14 使用它。

**兩組 API 分流**：`POST /agent/v1/tools/{工具}` 只接受 Agent 角色，管理 API（`/api/v1/…`）只接受桌面角色；`guard` 依路徑前綴 `/agent/v1/` 決定所需角色，角色不符回 403 `role_not_allowed`，Agent 憑證沒有範圍回 401 `credential_invalid`。身分與資源歸屬一律取自憑證；請求可以選擇帶 `workspace_id`／`run_id`，與憑證不一致就回 403 `agent_scope_mismatch`，不會悄悄改用請求裡的值。

| 工具 | 請求 | 行為 |
| --- | --- | --- |
| `get_task_contract` | `{}`（沒有必填欄位，但本文必須是 JSON 物件） | 回傳這個 Run **派工當下固定**的契約版本（`contract_version`、`contract`、`generation`）；Run 已結束或被接替回 401 `credential_revoked` |
| `report_progress` | `{ "summary" }` | 記錄最新進度 |
| `save_checkpoint` | `{ "summary", "details"? }` | 記錄 checkpoint；`details` 只存在稽核紀錄，事件只記摘要 |
| `report_blocked` | `{ "reason" }` | Task 從 `running` 進入 `blocked`；Run 本身不結束，Task 已不是 `running` 時不動 |

三個回報工具成功時回 `{ "accepted": true, "kind" }`，同時續約 lease，並追加 Run 事件 `run_progress_reported`、`run_checkpoint_saved`、`run_blocked_reported`。錯誤：內容無效 400 `report_invalid`（空白或超過上限；上限 `SUMMARY_MAX_CHARS`、`REASON_MAX_CHARS`、`DETAILS_MAX_BYTES` 見 `baton_domain::agent`）、JSON 不合法 400 `bad_request`。

**過期世代**：Run 已結束或已被接替之後的回報回 409 `run_report_stale`，**不改變任何目前狀態**（Task、lease、事件都不動），但內容仍以 `accepted = 0` 留在追加式資料表 `agent_report`（觸發器禁止更新與刪除）供稽核；被接受的回報同樣留在該表（`accepted = 1`）。

**面板**：`RunView`（`GET runs`、`GET tasks` 的 `runs`）多了 `latest_progress`、`latest_checkpoint`、`blocked`，各取該 Run 最新一筆**被接受**的回報；工作面板（`TaskBoard.tsx`）的卡片顯示它們。

**MCP shim**（`crates/baton-mcp-shim`）：手寫的 stdio JSON-RPC 2.0（逐行；stdout 只輸出協定訊息，日誌走 stderr），只依賴 `baton-domain`，不依賴 `baton-api` 或 Runtime。`serve(input, output, ShimConfig)` 可在程序內以任意讀寫串流測試。啟動時從環境變數讀 `BATON_AGENT_TOKEN`、`BATON_RUNTIME_URL`，缺少或空白時只印變數名稱並以 exit code 2 結束；輸出（含錯誤與 `Debug`）不含憑證。

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

### 共同設定與 Run 設定快照

所有帳號的 Agent 應該有一致的行為基準，但每個 Profile 的 `CLAUDE_CONFIG_DIR` 只承載登入與 Session 狀態（spec D-09、使用者故事 6）。所以共同設定從使用者平常使用的 Claude Code 設定**匯入**，使用者在預覽中看到來源、排除項目與差異並**採用**後，成為不可變的**版本**；每個 Run 啟動時再把它准入的版本寫成該 Run 專屬的唯讀快照目錄。規則在 `baton_domain::shared_settings`（純邏輯），讀檔走 `UserSettingsSource` Port，持久化在 `baton-store` 的 `shared_settings.rs`，端點在 `baton-api` 的 `shared_settings.rs`。使用者的原始設定檔只被讀取、從不被修改（情境測試比對匯入前後的位元組）。

**資料來源（白名單）**：`UserSettingsSource` 只讀下列三個檔案，Port 上沒有寫入能力，也沒有「列出目錄」之類的入口。

| 檔案 | 取用內容 |
| --- | --- |
| `~\.claude\settings.json` | 偏好、hooks、plugins 與市集 |
| `~\.claude\CLAUDE.md` | 全域指示（全文） |
| `~\.claude.json` | **只取 `mcpServers`** |

Runtime 的環境設了 `CLAUDE_CONFIG_DIR` 時，改讀該目錄下的 `settings.json`、`CLAUDE.md` 與 `.claude.json`（Claude Code 在這種情況下把 `.claude.json` 放在設定目錄之內）。**`.credentials.json` 永遠不讀；`.claude.json` 的其他欄位（登入帳號、使用者識別等）整份不讀**，不是讀進來再過濾。檔案不存在不是錯誤（該類別為空）；格式不是有效的 JSON 物件、不是 UTF-8 或預期是物件的欄位不是物件時，整次匯入失敗，不猜測、不部分匯入。

**匯入類別**（`SettingsBundle`，每類各自保存）：偏好（`settings.json` 中除下列以外的欄位，含 `permissions` 與 `env` 中非登入的變數）、指示（`CLAUDE.md`）、hooks（依事件名稱分項）、MCP 伺服器（依伺服器名稱分項）、plugins（`enabledPlugins`）與市集（`extraKnownMarketplaces`）。內容雜湊是排序後標準 JSON 的 SHA-256，與來源檔的排版、鍵順序無關。

**秘密處理**（匯入結果不得含任何登入秘密；測試以掃描驗證）：

- 登入相關欄位一律排除：`settings.json` 的 `apiKeyHelper`、`awsAuthRefresh`、`awsCredentialExport`、`forceLoginMethod`、`forceLoginOrgUUID`、`otelHeadersHelper`、`oauthAccount`、`primaryApiKey`、`customApiKeyResponses`；`env` 中名稱以 `ANTHROPIC_`、`CLAUDE_CODE_OAUTH`、`CLAUDE_CODE_USE_`、`AWS_`、`GOOGLE_APPLICATION_CREDENTIALS` 開頭的變數（這些會改變實際認證來源，讓 Profile 的原生登入失效）。原因代碼 `login_related`。
- **名稱像秘密**的字串欄位（名稱含 `apikey`、`secret`、`password`、`credential`、`authorization`、`cookie`、`token`（不含複數 `tokens`，例如 `MAX_OUTPUT_TOKENS` 是用量上限）等，忽略大小寫與 `-`、`_`）：只丟掉該欄位，其餘內容保留。原因代碼 `secret_key`。
- **內容像秘密**的字串（已知服務金鑰前綴如 `sk-ant-`、`ghp_`、`xoxb-`、`AKIA…`、JWT、`Bearer` 加長字串、PEM 私鑰、網址中的 `user:password@`、`--api-key=…` 這類參數）：該項目**整個排除**（整個 hook 事件、整個 MCP 伺服器、整份 `CLAUDE.md`），因為無法確定其餘內容是否安全。原因代碼 `secret_value`。
- `${VAR}` 形式的環境變數參照（可帶 `Bearer ` 前綴）視為非秘密並保留，因為它不含秘密本身。
- 排除紀錄只有「來源、項目路徑（例如 `env.ANTHROPIC_API_KEY`、`mcpServers.gh.env.GITHUB_TOKEN`）、原因」，**絕不含值**；來源只記錄檔案路徑與位元組的 SHA-256。

**限制（誠實說明）**：「名稱像秘密」與「內容像秘密」都是啟發式規則，只看形狀、不查證，**不保證抓得到所有秘密**。名稱不像秘密、內容也不符合已知格式的秘密（例如自訂格式的內部 token 放在 `CLAUDE.md` 或 hook 指令中）會被匯入。防線因此是三層：來源白名單（根本不讀登入檔）、登入欄位排除，最後才是啟發式遮蔽；採用前使用者應檢視預覽中的內容。

桌面憑證專用的端點（Agent 憑證得到 403）：

| 端點 | 用途 |
| --- | --- |
| `GET /api/v1/shared-settings` | 已採用的版本摘要（由舊到新，含各類別項目數與排除數）與最新版本 `current` |
| `POST /api/v1/shared-settings/import-preview` | 預覽匯入，**不保存任何東西**：來源（路徑、是否存在、SHA-256）、排除清單、候選內容、`candidate_hash`、與最新版本的差異（`diff`，每項有類別、名稱、新增／移除／變更與前後內容）、`changed` |
| `POST /api/v1/shared-settings/adopt` | `{ "expected_hash": <預覽的 candidate_hash> }`；重新讀取來源，雜湊相同才建立新版本，回 201 與完整版本 |
| `GET /api/v1/shared-settings/versions/{version}` | 某個版本的完整內容、來源與排除清單 |
| `POST /api/v1/workspaces/{id}/runs/{run_id}/settings-snapshot` | 為 Run 產生唯讀設定快照；JSON 本文 `{ "version"? }`（可為 `{}`，省略時用最新版本）；第一次建立回 201，之後回同一個綁定 200 |

錯誤一律是 `{ "error", "message" }`：

| 代碼 | 狀態 | 情境 |
| --- | --- | --- |
| `settings_source_changed` | 409 | 採用時來源已和預覽不同（`expected_hash` 不符）；請重新預覽、確認差異後再採用 |
| `settings_source_invalid` | 422 | 來源檔格式不是預期的（無效 JSON、不是物件、不是 UTF-8） |
| `settings_source_unreadable` | 500 | 來源檔存在但無法讀取（例如權限） |
| `settings_version_not_found` | 404 | 指定的版本不存在（讀版本、或快照指定了不存在的版本） |
| `shared_settings_not_adopted` | 409 | 還沒有採用任何版本，卻要求最新版本的快照 |
| `settings_snapshot_pinned` | 409 | Run 已固定使用某版本，要求改用別的版本 |
| `settings_snapshot_failed` | 500 | 無法寫入快照目錄 |

**版本不可變**：版本存在追加式的 `shared_settings_version` 表（版本號從 1 連續遞增；觸發器禁止更新與刪除），採用時在同一交易追加 `shared_settings_adopted` 事件（`aggregate_type = 'shared_settings'`，聚合版本 = 版本號）。**每次採用都建立新版本**，即使內容與目前版本相同（預覽的 `changed` 會告知這一點）；舊版本內容永遠不變。**採用必須帶預覽的 `candidate_hash`**：使用者採用的一定是他看過的內容，預覽之後來源檔有任何改變，採用就被拒絕。

**Run 設定快照**：`settings-snapshot` 把版本渲染成 `<資料目錄>\run-settings\<run_id>\`，只包含有內容的類別檔案，外加清單：

| 檔案 | 內容 |
| --- | --- |
| `settings.json` | 偏好、hooks、plugins 與市集 |
| `mcp.json` | `{ "mcpServers": … }` |
| `CLAUDE.md` | 全域指示 |
| `manifest.json` | 版本、內容雜湊與上列每個檔案的 SHA-256（供核對） |

渲染完全由版本內容決定（鍵排序、固定縮排），同一版本永遠得到相同的位元組；`bundle_from_snapshot_files` 可從目錄還原內容核對版本雜湊。寫入由 `WorkspaceFs::write_readonly_files` 完成：先寫到同層的 `<run_id>.staging`、每個檔案 `fsync` 並設為唯讀屬性，才改名成正式目錄，最後把目錄也設為唯讀，所以讀取端不會看到寫到一半的目錄。Run 與版本的綁定存在追加式的 `run_settings_snapshot` 表（觸發器禁止更新與刪除）：**Run 第一次產生快照時綁定版本，之後再採用新版本也不影響它**；再次呼叫回傳同一個綁定，要求別的版本回 409 `settings_snapshot_pinned`。快照端點目前由 API 呼叫，Run 准入流程尚未自動呼叫它。

> **Validation Required**（spec D-09）：
>
> - 快照目錄在 Windows 上只靠**檔案的唯讀屬性**保護：它能阻止直接覆寫檔案（情境測試驗證），但**不阻止刪除檔案**（實測 Rust 標準函式庫的刪除會忽略唯讀屬性，Node 的 `unlink` 也會先取消它）、**目錄本身無法阻止新增檔案**，唯讀屬性也不是安全邊界（同一 Windows 使用者可以取消它）。快照檔的內容雜湊記在 `manifest.json`，可用來偵測被竄改或刪除，但目前沒有任何流程自動核對（AI-99）。
> - Claude Code 是否能完全阻擋使用者層設定（`~\.claude\settings.json` 等）的合併與熱重載，**尚未實測**；在實測之前不可宣稱 Run 的設定已被固定。無法維持時，依 D-09 必須阻擋並顯示，不宣稱固定成功。
> - Adapter 啟動 Run 時如何以設定參數把快照檔傳給 Claude Code，**尚未接上**（屬後續票，正式執行檔的 AgentAdapter 仍是「尚未實作」）。

### 本機 API 的存取防護

Runtime 同時監聽 IPv4 與 IPv6 loopback（`127.0.0.1:<port>`、`[::1]:<port>`），拒絕非 loopback 的監聽位址。所有端點（含 `/api/v1/events` 串流與未知路徑）都要求：

- `Host` 是 loopback IP 字面值加上 Runtime 的埠；
- 若帶 `Origin`，必須是 Tauri App 的 origin（`http(s)://tauri.localhost`、`tauri://localhost`）；`OPTIONS`（CORS 預檢）一律拒絕、不回任何 `Access-Control-*` 標頭；
- 固定標頭 `X-Baton-Client: baton-desktop`；
- `Authorization: Bearer <憑證>`，且憑證的角色必須符合路徑：`/agent/v1/` 之下只接受 Agent 憑證，其餘（管理 API）只接受桌面憑證，角色不符回 403 `role_not_allowed`（見〈Agent API、範圍憑證與 MCP shim〉）。

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
- `harness.desktop_credential()`：與 App 相同，從 DPAPI 保護檔（測試中是 fake 保護器）讀出目前的桌面憑證；`harness.issue_credential(role, ttl)` 簽發額外憑證（Agent 角色或會過期的憑證，沒有範圍）；`harness.log_text()` 讀日誌。
- `harness.agent_credential(run_id)`：Run 的 Worker 啟動時收到的 Agent 憑證，取自 fake ProcessControl 記錄的啟動環境變數（與真實 shim 拿到的同一個值；Run 尚未啟動為 `None`）；`harness.runtime_url()`：Runtime 的 loopback 位址（shim 的 `BATON_RUNTIME_URL`）；`harness.call_tool(token, tool, body)`：以指定憑證呼叫 Agent 工具，回傳 `(狀態碼, JSON)`，不論成敗都不 panic。
- `harness.send_json(method, path, body)`：帶桌面憑證發出請求並回傳 `(狀態碼, JSON)`，用來斷言拒絕情境的錯誤代碼；`harness.db()`：以唯讀方式開啟控制資料庫，只用來確認「事件有追加、時間以 UTC 存放」這類必須看落地內容的事實。
- `Harness::builder().prepare_data_dir(..).try_start()`：Runtime 第一次啟動前先整理資料目錄（例如放入較新版本的資料庫），並回傳啟動錯誤而不是 panic。
- `harness.supervise()`：讓 Supervisor 處理一次（Harness 不啟動背景迴圈，何時發生由測試決定）；`process_control.hang_next_spawn_at(HangPoint::…)` 讓下一次 spawn 在「啟動前／啟動後回執前」永遠不回來，測試以 `timeout` 丟棄該次處理即等同 Runtime 在那一點崩潰；`agent_adapter.emit(run_id, event)`、`script_next_run(events)` 推送 Adapter 事件。
- `harness.restart()`：模擬重啟。舊的 Runtime 被丟棄（原位址不再回應），資料目錄與全部 fake 都保留。
- `harness.clock`、`process_control`、`agent_adapter`、`quota_observer`、`workspace_fs`、`platform_info`、`secret_protector`、`claude_env`：具體的 fake，可腳本化回應並讀取呼叫紀錄。`claude_env` 可設定 Claude Code／Git 是否存在與版本（`set_claude`、`set_git`、`fail_tool_probes`）、在某個設定目錄完成原生登入（`log_in`，會像真實的 Claude Code 一樣把登入資料寫進該目錄）、模擬環境變數覆蓋（`set_env_override`）、讓開啟登入終端機時順便登入（`script_terminal_login`），並記錄開啟過的終端機（`login_terminals`）。
- 示範測試：`crates/baton-harness/tests/health_demo.rs`；本機 API 防護情境：`crates/baton-harness/tests/api_auth.rs`；帳號 Profile 與首次啟動精靈情境：`crates/baton-harness/tests/profiles_and_first_run.rs`；Workspace 與控制資料庫情境：`crates/baton-harness/tests/workspaces.rs`；Task 與契約情境：`crates/baton-harness/tests/tasks.rs`；Supervisor、Run、程序身分與取消情境：`crates/baton-harness/tests/runs.rs`；Agent API、範圍憑證與 MCP shim 情境：`crates/baton-harness/tests/agent_api.rs`（以 `tokio::io::duplex` 當 stdio，在程序內跑真正的 shim 對真實 API 回報）。

後續的票撰寫情境測試時，一律透過這個 harness 操作真實 API；不要直接呼叫內部函式或斷言資料表欄位。

### CI

`.github/workflows/ci.yml` 在 `windows-latest` 上執行 `pnpm test` 與 `pnpm build`。

## 產品名稱

產品名稱集中在 `crates/baton-domain/src/product.rs`（`PRODUCT_NAME`、`DATA_DIR_NAME`）與安裝設定 `src-tauri/tauri.conf.json`（`productName`、視窗 `title`）。`crates/baton-domain/tests/product_name.rs` 會檢查兩處一致；Web UI 畫面上的名稱取自 Runtime 回報，不寫死。改名時只需修改這幾處（以及既有使用者的資料目錄）。

## 平台辨識與支援期限表

- PlatformInfo Port 只回報事實（OS build、功能版本、版本別、原生架構、程序架構）；支援狀態由 `baton-domain::platform::assess` 依事實與 Clock 判定，所以到期判斷可用 fake Clock 測試。
- 正式實作在 `crates/baton-runtime/src/windows_platform.rs`：Windows 11 以 build ≥ 22000 判定（登錄的 `ProductName` 在 Windows 11 上仍可能寫 Windows 10），原生架構取自 `IsWow64Process2`。
- 支援期限表（`SUPPORT_TABLE`）的來源與查閱日期寫在同一檔案；新增驗證通過的功能版本、或查到官方日期時，一併更新 `SUPPORT_TABLE_CHECKED_ON`。
