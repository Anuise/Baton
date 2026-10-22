# 開發者指南

Baton（暫名，見 spec D-01）是 Windows 桌面應用程式：Rust 寫的常駐 Runtime、React + TypeScript + Vite 的 Web UI、Tauri 桌面殼。舊版 skill（`skills/`、README）在重建期間不動，已用 git tag `archive/legacy-skill` 封存。

## 目錄

| 路徑 | 內容 |
| --- | --- |
| `crates/baton-domain` | Domain 核心（純邏輯、無 I/O）：產品名稱常數、時間戳、Runtime 設定解析、平台支援狀態判定與支援期限表（`platform.rs`）、Workspace 狀態機（`workspace.rs`：命令 → 新狀態 + 事件）、Workspace 授權範本／確認 revision／Run 准入檢查（`authorization.rs`）、Goal／Task 狀態機與六項最小派工契約（`task.rs`）、Run 狀態機／execution lease／派工 Outbox（`run.rs`）、多帳號排程與容量（`scheduler.rs`：選帳號、並行上限、交接槽、等待原因）、額度觀測與判斷（`quota.rs`：已觀測／推估、較保守者、觀測不完整）、account_profile 狀態機與可派工判定（`profile.rs`）、首次啟動精靈的工具檢查與背景行為告知（`setup.rs`）、資料目錄位置判定（`storage_location.rs`）、Agent 工具介面（`agent.rs`：環境變數名稱、MCP 設定、Worker 回報內容與長度上限）、Artifact 的登記驗證、版本 manifest、未登記檔案辨識與預覽判定（`artifact.rs`）、確定性檢查與成果接受交易（`verification.rs`）、共同設定的匯入、秘密遮蔽、差異、版本與 Run 快照渲染（`shared_settings.rs`，秘密辨識在 `shared_settings/secrets.rs`）、裁定請求與裁定的狀態機（`decision.rs`）、權限提示的歸類與授權判定（`permission.rs`）、單次批准（E1）的綁定與核對（`approval.rs`）、備份的觸發／保留／還原演練核對（`backup.rs`）、知識條目與教訓的可信狀態轉換、版本、取代關係、可見範圍與搜尋關鍵字（`knowledge.rs`）、Guard 規則、啟用停用、Run 准入時的違反判定與重犯事件（`guard.rs`）、桌面生命週期與完全退出（`lifecycle.rs`：停止意圖、停止報告與停止異常、各 Workspace 的自動續跑與暫停、生命週期檔）、Task 工作目錄（worktree／輸出目錄）的位置與登記（`workdir.rs`）、單一整合佇列的狀態機（`integration.rs`）、外部操作的冪等鍵、要求雜湊、回執與結果未知（`external_operation.rs`）、控制資料庫寫入故障的種類與處理方式（`storage_health.rs`） |
| `crates/baton-ports` | 所有外部副作用的 Port 介面：Clock、ProcessControl（含 `find`）、AgentAdapter（含 `spawn_spec`）、QuotaObserver、WorkspaceFs（含成果的 `write_artifact_atomic`、`read_artifact`、`list_artifact_files`，以及 `write_readonly_files`：把一組檔案寫成唯讀目錄）、PlatformInfo、SecretProtector、ClaudeEnvironment、CommandRunner（驗證命令）、UserSettingsSource（唯讀讀取使用者平常使用的 Claude Code 設定，只有白名單內的三個檔案）；`write_file_atomic`（暫存 → fsync → 原子改名）是成果寫入的真實實作 |
| `crates/baton-store` | SQLite 存取：單一寫入佇列、PRAGMA 讀回驗證、`schema_migrations`、Workspace、account_profile、授權 revision、Goal／Task／契約版本／相依與事件表、Run／lease／Outbox 表、派工佇列與容量設定（`scheduling.rs`）、成果 manifest（`artifact`、`artifact_version`）、驗證紀錄（`task_verification`）、共同設定版本表 `shared_settings_version` 與 Run 設定快照綁定表 `run_settings_snapshot`（都是追加式）、額度觀測／本機用量／方案上限（`quota.rs`）、裁定請求／裁定／單次批准（`decisions.rs`）、備份／還原／還原演練（`backup.rs`）、知識條目與版本（`knowledge.rs`：`knowledge_entry`、`knowledge_entry_version`，以及每次開啟時重建的全文檢索索引 `knowledge_fts`）、Guard 與重犯事件（`guards.rs`：`guard`、`lesson_recurrence`）、停止意圖與 Workspace 生命週期設定、生命週期檔的原子讀寫（`lifecycle.rs`）、外部操作（`external_operations.rs`：`external_operation`，Run 結束與回答裁定的同一個交易中結算）、寫入故障的歸類與試寫（`health.rs`：`storage_probe`） |
| `crates/baton-api` | Axum 路由（`/api/v1/health`、`/api/v1/platform`、`/api/v1/workspaces`（含授權、准入檢查、Goal、Task、Run、Run 設定快照、成果瀏覽與成果接受）、`/api/v1/scheduling`、`/api/v1/lifecycle`、`/api/v1/decisions/pending`（與 `/api/v1/workspaces/{id}/decisions`）、`/api/v1/profiles`、`/api/v1/setup`、`/api/v1/shared-settings`（共同設定的預覽、採用與版本）、`/api/v1/backups`（備份與還原演練狀態）、`/api/v1/knowledge`、`/api/v1/guards`（與 `/api/v1/knowledge/{id}/recurrences`）、`/api/v1/events`）與入口防護 `guard`（憑證、角色、Host、Origin、CORS）；Agent API（`/agent/v1/tools/…`，`agent.rs`，含 `register_artifact`；`knowledge.rs` 的 `record_lesson_candidate`、`search_knowledge`） |
| `crates/baton-credential` | 桌面憑證檔（`auth/desktop-credential.bin`）的讀寫與 DPAPI 保護 |
| `crates/baton-runtime` | Runtime 組裝（`Runtime::start`，`RuntimeOptions` 含 `full_text_search`；含啟動時核對停止意圖與完全退出的執行者）、Supervisor（`supervisor.rs`，含完全退出的停止流程與重新開啟）、Claude Code Adapter（`claude_adapter.rs`）、備份排程（`backup.rs`）、確定性檢查的執行（`verifier.rs`）、Windows Job Object 的 ProcessControl（`windows_process.rs`）、單實例鎖、`baton-runtime.exe`（`baton-runtime restore …` 還原備份） |
| `crates/baton-mcp-shim` | MCP 工具 shim（`baton-mcp-shim.exe`）：Claude Code 以 stdio 啟動，把工具呼叫轉送到 Runtime 的 Agent API；只依賴 `baton-domain` |
| `crates/baton-harness` | 測試 harness 與全部 Port 的 fake（主要測試 seam，spec D-04） |
| `crates/baton-client` | 桌面殼用的 `RuntimeClient`（自動讀取憑證）與「連回既有 Runtime 或啟動新的」邏輯 |
| `src-tauri` | Tauri 桌面殼（系統匣、關窗隱藏）與安裝設定（`tauri.conf.json`） |
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
| `POST /api/v1/workspaces/{id}/authorization/summary` | 確認前的摘要（預覽，不保存）；含「可信程式以目前 Windows 使用者身分執行」的風險。已有生效的授權時另有 `changes`：相對於目前 revision 新增／移除的能力、目標與限制，`expands`（是否擴大授權）與 `impact`（影響說明） |
| `PUT /api/v1/workspaces/{id}/authorization` | 確認並保存：新內容回 201 與新的 revision（含來源與時間）；內容與目前相同回 200；需要承認風險時必須帶 `acknowledged_risk: true`；修改既有授權而且擴大（新增能力、目標或移除限制）時必須帶 `acknowledged_expansion: true`，否則 400 `authorization_expansion_not_acknowledged`（第一次確認與縮小授權不需要，AI-183） |
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

**Run 狀態**：`queued → starting → running → succeeded／failed／interrupted／cancelled`（`RunStatus::allowed_next`）；另有 `queued → failed`（寫入 launch intent 時准入已不成立，從未啟動程序）。只有 `running` 之後才有程序身分；`starting` 代表 launch intent 已持久化但回執還沒寫回。

**派工交易**（Domain `RunCommand::Dispatch`，帶 Scheduler 選定的 `profile_id`）：在同一個交易中重新做完整准入檢查（`run::check_dispatch`：`task::admission_report`、`authorization::admit`、適用的 Guard 都沒有違反（見〈Guard 與執行前檢查〉）、相依都已完成、沒有任何尚未釋放的 lease），然後建立 Run（`queued`，記下使用的帳號）、execution lease（世代 = 該 Task 的 Run 數 + 1）、派工 Outbox 項目，Task 進入 `running`。派工**不啟動程序**。誰來選帳號、容量不夠時怎麼辦，見下一節。

**寫入所有權**：同一個 Task 任何時刻最多一個尚未釋放的 `execution_lease`（Domain 判定 + 資料庫唯一索引 `execution_lease_one_held_per_task`；性質測試 `at_most_one_effective_writer_per_task_under_any_command_sequence`）。Run 結束或取消**確認程序停止之後**才釋放；停不掉（`StopResult::StillAlive`）就一直持有，新的 Run 派不出去。

**Supervisor**（`Supervisor::tick`，每次依序做七件事，全部冪等、只信任資料庫）：

1. 啟動派工 Outbox 的 Run（Outbox Dispatcher，至少一次交付、以 `command_id` 去重）：先在交易中持久化 launch intent 與 `command_id`（`starting`），才在交易外 `spawn`，再把回執（PID + 建立時間 + 指令雜湊）寫回（`running`）。寫入 launch intent 的交易再檢查一次停止意圖（`launch_halted`）與**准入**（目前生效的授權仍涵蓋契約需要的能力、Workspace 沒有封存）；准入不成立時 Run 以 `failed`（`launch_admission_denied：…`）結束、Outbox 視為處理完畢，不簽發憑證也不啟動程序。`spawn` 回報失敗時先以 `command_id` 呼叫 `ProcessControl::find` 查證：找到（例如回執在傳回途中遺失）就補寫回執；確定沒有才判定 `launch_failed`；無法查證就留在 `starting` 等下一次核對。Runtime 在任何一點當掉，重建後以 `command_id` 找回原程序並補寫回執，**不重複啟動**；找不到就把 Run 標為 `interrupted`（`launch_outcome_unknown`），不自動重送，由使用者重新派工；`find` 本身失敗（無法證明程序在不在）時 Run 留在 `starting`、繼續持有寫入所有權，**不中斷也不重派**，直到查得到為止。控制資料庫無法寫入時（見〈外部操作與寫入故障〉）這一步與第 6 步都暫停。
2. 收 Adapter 事件：`Heartbeat` 續約 lease（間隔 15 秒、有效 60 秒，spec D-31）；`RateLimited`、`UsageReported` 記為該 Run 帳號的額度觀測（見「額度觀測」）；`Exited` 只結束 Run（成功 → Task `verifying`，失敗 → Task 回到 `ready`），**不會**把 Task 標為 `completed`。
3. 取消：要求取消的 Run 先記下意圖（`cancel_requested_at`），Supervisor 停止整個程序樹，確認之後才完成取消、釋放 lease、Task 回到 `ready`；還沒啟動的 Run 立即取消。取消 Task 會在同一個交易中要求取消它進行中的 Run。
4. 核對 lease：過期只觸發核對（`inspect`）：確認 `Dead` 才中斷 Run，`Alive` 續約，`Unknown` 維持原狀。
5. 額度：輪詢到期（每個帳號每 5 分鐘）的外部額度觀測器並重新推估（見「額度觀測」）。
6. 排程：前面幾步可能釋放了名額，於是讓派工佇列依序再試一次（見下一節）；派出的 Run 在同一次 `tick` 就啟動。
7. 驗證：對 `verifying` 且契約宣告了確定性檢查的 Task 執行檢查並提交結果（見「成果接受與最小完成驗證」）；背景迴圈把這一步放在另一個工作執行。

正式執行時由背景迴圈每秒（或派工／取消時被喚醒）呼叫 `tick`；`RuntimeOptions { background_supervisor: false }`（harness 使用）不啟動背景迴圈，由測試呼叫 `harness.supervise()`。`RuntimeOptions::stop_grace` 預設 30 秒（spec D-11）。

**真實 ProcessControl**（`WindowsProcessControl`，只在 Windows 編譯）：每個 Run 一個具名 Job Object `Local\Baton-Run-{command_id}`（不設 KILL_ON_JOB_CLOSE，Runtime 崩潰不會殺掉程序）；`stop` 先在寬限期內等程序自行結束，逾時 `TerminateJobObject` 終止整棵樹。整合測試：`crates/baton-runtime/tests/process_tree.rs`（直接測 Port）、`crates/baton-runtime/tests/supervisor_process_tree.rs`（真實 Runtime + HTTP 取消）。正式執行檔的 AgentAdapter 是 Claude Code Adapter（見「Claude Code Adapter」）。

桌面憑證專用的端點（Agent 憑證得到 403），在 `/api/v1/workspaces/{id}/` 之下：

| 端點 | 用途 |
| --- | --- |
| `POST tasks/{task_id}/dispatch` | 派工要求（Task 必須是 ready，本文可帶 `{"priority": n}`）；有帳號與名額 → 201 回 `queued` 的 Run（含 `profile_id`）；否則 202 回 `{queued: true, reasons}`，Task 留在派工佇列。409 代碼：`task_not_ready`、`task_admission_denied`（契約失效，帶 `missing`）、`execution_conflict` |
| `GET runs`、`GET runs/{run_id}` | Run（含 lease、程序身分、`cancel_requested_at`、`end_reason`）；工作面板 `GET tasks` 也帶 `runs` |
| `POST runs/{run_id}/cancel` | 要求取消；已結束或重複取消是冪等的（200） |

事件（`aggregate_type = 'run'`）：`run_created`、`run_launch_started`、`run_started`、`run_lease_verified`、`run_succeeded`、`run_failed`、`run_interrupted`、`run_cancel_requested`、`run_cancelled`（heartbeat 只更新 lease，不追加事件）。

### Agent API、範圍憑證與 MCP shim

Worker 不直接碰資料庫或管理 API：它的程序啟動時帶著一組**只屬於這個 Run** 的憑證，由 MCP shim 把工具呼叫轉送到 Runtime 的 Agent API。純邏輯在 `baton_domain::agent`，端點在 `baton-api` 的 `agent.rs`，持久化在 `baton-store` 的 `agent.rs`。

**憑證怎麼交給程序**：Supervisor 在 `BeginLaunch`（launch intent 已持久化）之後、`spawn` 之前，呼叫 `CredentialIssuer::issue_agent` 為該 Run 簽發 Agent 憑證，綁定 Workspace、Run 與世代。它沒有時間上限，有效與否由「這個 Run 仍是寫入所有者」決定（所以 Runtime 重啟後，Worker 程序手上的憑證仍可用）。簽發失敗就不啟動程序。憑證只經 `SpawnSpec.env` 的環境變數 `BATON_AGENT_TOKEN` 與 `BATON_RUNTIME_URL` 交給程序，**不**進 Prompt、指令列、MCP 設定檔、日誌或資料庫明文（資料庫只存雜湊）。`SpawnSpec` 的 `Debug` 只印環境變數名稱，環境變數不納入 `command_hash`；真實 `WindowsProcessControl` 以 Unicode 環境區塊傳入（同名時以 `spec.env` 為準）。

**MCP 設定**：`baton_domain::agent::mcp_config_json(shim 路徑)` 產生的設定只含 shim 指令與 `${BATON_AGENT_TOKEN}`、`${BATON_RUNTIME_URL}` 的參照，由 Claude Code 展開；真實 AgentAdapter 為每個 Run 把它寫成 `runs\<run_id>\mcp.json`。

**兩組 API 分流**：`POST /agent/v1/tools/{工具}` 只接受 Agent 角色，管理 API（`/api/v1/…`）只接受桌面角色；`guard` 依路徑前綴 `/agent/v1/` 決定所需角色，角色不符回 403 `role_not_allowed`，Agent 憑證沒有範圍回 401 `credential_invalid`。身分與資源歸屬一律取自憑證；請求可以選擇帶 `workspace_id`／`run_id`，與憑證不一致就回 403 `agent_scope_mismatch`，不會悄悄改用請求裡的值。

| 工具 | 請求 | 行為 |
| --- | --- | --- |
| `get_task_contract` | `{}`（沒有必填欄位，但本文必須是 JSON 物件） | 回傳這個 Run **派工當下固定**的契約版本（`contract_version`、`contract`、`generation`）；Run 已結束或被接替回 401 `credential_revoked` |
| `report_progress` | `{ "summary" }` | 記錄最新進度 |
| `save_checkpoint` | `{ "summary", "details"? }` | 記錄 checkpoint；`details` 只存在稽核紀錄，事件只記摘要 |
| `report_blocked` | `{ "reason" }` | Task 從 `running` 進入 `blocked`；Run 本身不結束，Task 已不是 `running` 時不動 |
| `register_artifact` | `{ "name", "description"?, "content"? \| "content_base64"? }` | 登記成果並回 201 `{ artifact_id, version, name, size_bytes, sha256, created }`；內容以參數內嵌傳入、不讀取任何路徑。細節見〈成果儲存與成果瀏覽〉 |
| `record_lesson_candidate` | `{ "title", "body"?, "trigger"?, "root_cause"?, "resolution"?, "tags"?, "evidence"?, "scope"? }` | 提交**候選**教訓並回 201 `{ entry_id, status, revision }`；狀態一律是 `candidate`。細節見〈知識、教訓與全文檢索〉 |
| `search_knowledge` | `{ "query"? }` | 只回傳這個 Run 看得到、**有效**的條目。細節見〈知識、教訓與全文檢索〉 |
| `begin_external_operation` | `{ "idempotency_key", "operation", "target", "request"? }` | 送出部署、發訊、付費這類外部操作**之前**登記；只有 `status: "proceed"` 才可以送出。細節見〈外部操作與寫入故障〉 |
| `record_external_operation_result` | `{ "idempotency_key", "result": "succeeded"\|"failed"\|"unknown", "receipt"?, "reason"? }` | 回報結果；`unknown` 建立裁定請求並讓 Task 等待查證。細節見〈外部操作與寫入故障〉 |

三個回報工具成功時回 `{ "accepted": true, "kind" }`，同時續約 lease，並追加 Run 事件 `run_progress_reported`、`run_checkpoint_saved`、`run_blocked_reported`。錯誤：內容無效 400 `report_invalid`（空白或超過上限；上限 `SUMMARY_MAX_CHARS`、`REASON_MAX_CHARS`、`DETAILS_MAX_BYTES` 見 `baton_domain::agent`）、JSON 不合法 400 `bad_request`。

**過期世代**：Run 已結束或已被接替之後的回報回 409 `run_report_stale`，**不改變任何目前狀態**（Task、lease、事件都不動），但內容仍以 `accepted = 0` 留在追加式資料表 `agent_report`（觸發器禁止更新與刪除）供稽核；被接受的回報同樣留在該表（`accepted = 1`）。

**面板**：`RunView`（`GET runs`、`GET tasks` 的 `runs`）多了 `latest_progress`、`latest_checkpoint`、`blocked`，各取該 Run 最新一筆**被接受**的回報；工作面板（`TaskBoard.tsx`）的卡片顯示它們。

**MCP shim**（`crates/baton-mcp-shim`）：手寫的 stdio JSON-RPC 2.0（逐行；stdout 只輸出協定訊息，日誌走 stderr），只依賴 `baton-domain`，不依賴 `baton-api` 或 Runtime。`serve(input, output, ShimConfig)` 可在程序內以任意讀寫串流測試。啟動時從環境變數讀 `BATON_AGENT_TOKEN`、`BATON_RUNTIME_URL`，缺少或空白時只印變數名稱並以 exit code 2 結束；輸出（含錯誤與 `Debug`）不含憑證。

### 裁定中心與權限提示轉接

Worker 需要使用者決定的事，以及超出授權範圍的操作，都變成**裁定請求**，集中在裁定中心等使用者回答。純邏輯在 `baton_domain::decision`（狀態機）與 `baton_domain::permission`（權限提示判定），持久化在 `baton-store` 的 `decisions.rs`，端點在 `baton-api` 的 `decisions.rs`（桌面）與 `agent.rs`（Agent 工具），UI 在 `web/src/DecisionCenter.tsx`。

**四條不變條件**（每一條都有情境測試，`crates/baton-harness/tests/decisions.rs`）：

1. **Agent 不能裁定**：回答只有桌面憑證的端點（`/api/v1/…`，Agent 憑證在 `guard` 就得到 403 `role_not_allowed`）；Agent API 沒有任何回答或批准的工具；`decision.decided_by` 在資料庫層 CHECK 為 `desktop`。
2. **只阻擋受影響的 Task**：請求的 `blocking` 明列受影響的 Task，只有它們進入 `waiting_decision`；回答後只有仍在 `waiting_decision` 而且**沒有其他待處理請求也阻擋它**的 Task 回到 `ready`，並在同一個交易重新排進派工佇列（`dispatch_request`，優先序 0），由 Scheduler 自動派出新的 Run；舊 Run 還持有 execution lease 時留在佇列等待（原因 `execution_conflict`），舊 Run 結束後的下一次排程派出。其他 Task 從頭到尾不受影響。
3. **逾時不是同意**：`deadline_minutes` 只讓 `overdue` 變成 `true`；沒有任何程式路徑會因時間流逝回答請求、放行權限提示或恢復 Task。
4. **一次交易**：回答時，裁定、請求狀態、受影響 Task 的狀態與事件、重新排隊、喚醒 Outbox 在同一個交易提交。

**Agent 工具**（`POST /agent/v1/tools/{工具}`，範圍取自憑證，只有仍持有 execution lease 的 Run 能呼叫；否則 409 `run_report_stale`）：

| 工具 | 請求 | 行為 |
| --- | --- | --- |
| `request_decision` | `question`、`reason`、`options`（2–8 個 `{id, label, tradeoff?}`）、`recommendation?`、`evidence?`、`urgency?`（`low`／`normal`／`high`）、`deadline_minutes?` | 建立請求；Task（`running` 時）進入 `waiting_decision`，Run 不結束。回 `{decision_request_id, status: "waiting_decision", existing, message}`；同一個 Run 對同一個問題重複提出時回到既有請求。內容無效 400 `decision_invalid`，不寫入任何東西 |
| `permission_prompt` | `tool_name`、`input`（Claude Code `--permission-prompt-tool` 協定） | 授權範圍內：回 `{behavior: "allow", updatedInput, status: "auto_allowed"}` 並追加稽核事件 `permission_auto_allowed`。範圍外：建立裁定請求（選項固定為 `allow_once`、`deny`）並回 `{behavior: "deny", message: "等待裁定…", status: "waiting_decision", decision_request_id}`。使用者已回答時，同一個 Task 的同一個操作消耗一次答案：`approved`（允許）或 `denied`（拒絕）。先前的單次批准不能用時，等待裁定的回應另帶 `approval_check: {decision_request_id, result}`（`params_changed`、`target_changed`、`artifact_version_changed`…或 `used_result_unknown`） |

**權限提示的判定**（`permission::evaluate`，保守的詞法判定，細節與取捨見 AI-138）：把「工具名稱＋輸入」歸類為能力（讀取／編輯檔案、讀取公開網頁、建置測試、安裝專案依賴、全域安裝、遠端寫入），再對照 Workspace **目前生效的授權 revision**；檔案路徑必須落在授權目錄內，寫入 `.git`、本機或內網網址、含 shell 組合語法的指令、未知工具、尚未確認授權一律範圍外。Bash 指令以 bash 規則斷詞（去掉引號；引號不成對或反斜線緊接引號一律無法歸類），每個子命令只接受白名單內的旗標，任何其他旗標（例如 `git commit --amend`、`git diff --output=…`、`npm install --prefix …`、`cargo test --manifest-path …`）都無法歸類；`-g`、`--global=…`、`--location=global` 等全域安裝的各種寫法歸為「全域安裝」；指令參數中像路徑的字詞必須在 Workspace 內。路徑元件比對前依 Win32 規則去掉結尾的 `.` 與空白（`.git.`、`.git ` 視同 `.git`），含 `:`（替代資料流）的元件、只有點的元件、`\\?\` 裝置路徑一律範圍外，`GIT~1` 視同 `.git`。單次批准的規則見 AI-139。

**單次批准（E1，票 23）**（`baton_domain::approval`，細節與取捨見 AI-180～AI-182）：權限提示的請求建立時就算好它綁定的**單一邏輯操作**——操作識別（工具名稱，Bash 另加程式與子命令，例如 `Bash:git push`）、目標資源（檔案路徑、網址，或指令中非旗標的參數）、重要參數（其餘輸入與旗標）、要求雜湊（工具名稱與輸入的 SHA-256）、成果版本（提出請求的 Task 已登記成果的最新版本，`名稱@版本`）；Claude Code 附上的 `description` 說明欄位不列入。「允許本次操作」只綁 Task，不綁 Run 或帳號，所以恢復後換帳號、換 Run 的 Worker 可以沿用；Worker 再次提示時**逐項核對**，任何一項不同就不消耗批准、改建新的請求，並在回應與新請求的 `permission.rejected_approval` 說明哪一項變了。批准用過一次就失效；之後再提出同一個操作時，Runtime 看不到上一次的結果，新的請求帶 `permission.prior_use`、建議拒絕、選項改為「已查證，允許再執行一次」，要求使用者先查證。單次批准從不改變 Workspace 授權；要永久放行得走「修改 Workspace 授權」（摘要的 `changes` 與 `acknowledged_expansion`），修改授權不會回答裁定請求。

**桌面端點**（桌面憑證專用）：

| 端點 | 用途 |
| --- | --- |
| `GET /api/v1/workspaces/{id}/decisions` | `{pending_count, decisions}`；待處理在前（緊急程度高者優先），已回答在後 |
| `GET …/decisions/{request_id}` | 單筆詳情：問題、原因、選項與取捨、建議、證據、阻擋範圍（受影響的 Task 與狀態）、權限提示細節（工具、輸入、能力、單次批准的 `binding`、`prior_use`、`rejected_approval`、`approval_use`）、期限與 `overdue`、已回答時的裁定 |
| `POST …/decisions/{request_id}/answer` | `{option_id, note?}`；200 回 `{decision, resumed_task_ids}`。同樣的答案重送是冪等的；不同的答案 409 `decision_already_answered`；未知選項 400 `decision_option_unknown`；找不到 404 `decision_not_found` |
| `GET /api/v1/decisions/pending` | `{pending_count, by_workspace}`，給狀態列的「待裁定數」 |

資料表（migration 15）：`decision_request`（只能由 `pending` 改成 `answered`）、`decision_request_block`、`decision`（追加式）、`decision_approval_use`（追加式）。回答時追加的喚醒 Outbox 項目種類是 `decision_wake:<請求識別>`，**目前沒有消費者**（Controller 的事件迴圈在後續的票）。事件：`decision_requested`、`decision_answered`、`permission_approval_used`（聚合 `decision_request`），以及自動允許的 `permission_auto_allowed`（每筆一個 `permission_audit` 聚合）。

本票只做最小版本：沒有 `expected_revision`／過期標記、批次回答、相似請求合併、決策權限表的強制檢查（AI-143）。Adapter 以 `--permission-prompt-tool mcp__baton__permission_prompt` 接上；回應形狀已以 Claude Code 2.1.307 實測（CLI 送來 `tool_name`、`input`、`tool_use_id`，接受帶 Baton 說明欄位的 `deny`，被拒絕的工具不執行、Worker 不重試同一個操作；見 `docs/adapter-contract.md`）。

### 外部操作與寫入故障

Runtime 無法撤回的外部操作（部署、發訊、付費、遠端寫入）由 Worker 執行，但**送出前後都要向 Runtime 登記**，Runtime 據此保證「結果未知時不盲目重送」（GLOSSARY〈Unknown Side Effect〉）。純邏輯在 `baton_domain::external_operation`，持久化在 `baton-store` 的 `external_operations.rs`，端點在 `baton-api` 的 `external_operations.rs`。

**登記與結果**：Worker 以 `begin_external_operation` 登記冪等鍵（範圍是 Workspace）、操作種類、目標與參數；Runtime 以操作、目標與參數算出要求雜湊（不含冪等鍵；物件鍵的順序不影響）。回應一律 200，Worker 依 `status` 決定下一步：

| `status` | 意義 | Worker 該做的事 |
| --- | --- | --- |
| `proceed` | 新的鍵，或先前確定沒有生效（第 N 次嘗試，`attempt`）；同一個 Run 重送登記回 `existing: true` | 送出，之後回報結果 |
| `already_succeeded` | 先前已經成功（附 `receipt`） | 不要再送，沿用回執 |
| `result_unknown` | 先前送出後結果未知，正在等查證（附 `decision_request_id`） | 不要送；保存 checkpoint 後 `report_blocked`，或改做其他步驟 |
| `in_progress_elsewhere` | 另一個仍在執行的 Run 正在進行同一個操作 | 不要送 |

同一個鍵、內容不同回 409 `idempotency_key_conflict`，原紀錄不變。`record_external_operation_result` 的 `succeeded` 必須附 `receipt`、`failed`／`unknown` 必須附 `reason`；只能回報自己這個 Run 登記、仍在進行的操作（否則 409 `external_operation_not_in_progress`，找不到 404 `external_operation_not_found`）；重送同樣的結果是冪等的，改成別的結果 409 `external_operation_already_recorded`。Run 已結束或被接替 409 `run_report_stale`。

**結果未知**：以下兩種情形，操作變成 `unknown`，在**同一個交易**建立裁定請求（種類 `agent`、緊急程度 `high`，識別 `decision_<操作>_a<嘗試>`，選項 `verified_done`「已查證：操作已經生效」與 `verified_not_done`「已查證：操作沒有生效」，不給建議），相關 Task 從 `running`／`ready`／`verifying` 進入 `waiting_decision`：

1. Worker 回報 `unknown`（逾時、連線中斷等無法確定的情形）。
2. Run 結束時還有沒回報結果的操作——不論是正常結束、核對後中斷（含崩潰重建後）、取消、完全退出或還原。這一步掛在 `runs::save` 裡（所有 Run 的結束都經過它），所以不會有「Run 已結束、Task 回到 ready 被重新派出，操作卻還沒標成結果未知」的空檔。

結果未知期間，同一個鍵的登記一律回 `result_unknown`；別的 Task 用了同一個鍵也一樣，而且該 Task 被加入同一個請求的阻擋範圍，一起等查證。其他 Task 不受影響。使用者回答時，查證結果與 Task 恢復（回到 `ready` 並重新排隊）在同一個交易提交：`verified_done` → 操作 `succeeded`（回執記為「使用者查證已生效」加備註），之後同一個鍵回 `already_succeeded`；`verified_not_done` → 操作 `failed`，之後同一個鍵可以再執行一次（嘗試次數加一）。

桌面端點 `GET /api/v1/workspaces/{id}/external-operations` 回傳 `{ operations }`（依建立順序，含狀態、嘗試次數、回執、原因與裁定請求）。事件（`aggregate_type = 'external_operation'`）：`external_operation_begun`、`external_operation_succeeded`、`external_operation_failed`、`external_operation_result_unknown`、`external_operation_verified`。資料表（migration 22 `external_operation`）：`external_operation`（`(workspace_id, idempotency_key)` 唯一、不可刪除）與 `storage_probe`。

**寫入故障（磁碟滿、資料庫被鎖住）**：Store 的寫入佇列依 SQLite 錯誤代碼歸類每個失敗的寫入（`baton_store::storage_fault_of`）：`SQLITE_FULL` → `disk_full`、`SQLITE_BUSY`／`SQLITE_LOCKED` → `busy`、I/O 錯誤或唯讀 → `unwritable`；其他錯誤（例如約束違反）不算。有故障時：

- Supervisor 不啟動任何程序（第 1 步）也不排程（第 6 步），每次處理先試寫 `storage_probe`；試寫成功才解除，沒有寫入任何東西的交易不算恢復。
- 會寫入的 API 回 503，錯誤代碼是故障種類（`storage_disk_full`、`storage_busy`、`storage_unwritable`），訊息附處理方式。
- `GET /api/v1/health` 的 `storage` 是 `{ dispatch_halted, fault: { kind, code, summary, action, detail } | null }`；Web UI 的狀態列（`StatusBar.tsx`）在 `dispatch_halted` 時顯示「新派工已停止」、原因與處理方式。
- 程序已啟動、回執卻因磁碟滿寫不回去時，Run 留在 `starting`；恢復可寫之後以 `command_id` 找回同一個程序補寫回執，不會啟動第二個。

### 多帳號並行排程與容量

規則在 `baton_domain::scheduler`（純邏輯：事實 → 要派給誰或為什麼等），持久化在 `baton-store` 的 `scheduling.rs`，端點在 `baton-api` 的 `scheduling.rs` 與 `runs.rs`，佇列的定期重試在 `Supervisor::schedule_queue`。

**一個派工要求怎麼處理**（`Store::schedule`，單一交易）：先用目前狀態判定一次，「不是等一等就好」的錯誤（Task 不是 ready、找不到、契約失效）原樣拒絕且什麼都不寫；否則記入 `dispatch_request` 佇列，再依「優先序高者先、同優先序先要求者先」逐一嘗試派出每個排隊中的 Task（每個 Task 各自判定，被相依擋住的不會擋住後面的）。派出 = 選帳號 + `RunCommand::Dispatch`；等待 = 留在佇列。Task 離開 `ready`（派出、取消、改契約）時 `save_task` 一併把它移出佇列。Run 失敗後 Task 回到 `ready`，**不會**自動重新排隊（沒有重試預算前不自動重派）；裁定後恢復的 Task 例外，回答時就重新排隊（AI-140）。

**容量**（`CapacityMode`，設定頁切換，`PUT /api/v1/scheduling/settings`）：

| 預設 | 每帳號 | 全域 | 交接槽 |
| --- | --- | --- | --- |
| `reserve_handoff`（預設，spec D-14） | 2 | 6 | 可用帳號數 ≥ 3 時保留 1 個，一般派工最多 5 個 |
| `higher_concurrency` | 3 | 8 | 不保留 |

進行中 = 尚未結束的 Run（`queued`／`starting`／`running`，含已要求取消但程序還沒確認停止者）。`SlotKind::Handoff` 可以使用保留槽，目前只有 Domain 單元測試使用它，交接（票 28）才會真的用到。選帳號：核對無誤、額度未達門檻、帳號名額未滿者中，進行中 Run 最少的；同數時取較早建立的帳號。

**等待原因**（`WaitReason`，依類別分開，讓使用者知道該處理哪一類問題）：

| `category` | 代表 | 例（`code`） | 怎麼恢復 |
| --- | --- | --- | --- |
| `capacity` | 名額已滿 | `account_capacity_full`、`global_capacity_full`、`handoff_slot_reserved` | 名額空出來自動派出 |
| `quota` | 帳號的判斷用量（較保守者）≥ 75% | `quota_near_limit` | 等重置（遲滯與 draining 屬票 28）；用量未知**不**阻擋 |
| `authorization` | Workspace 授權不足或尚未確認 | `capability_not_granted`、`authorization_unconfirmed` | 使用者重新確認授權 |
| `dependency` | 相依的 Task 還沒完成 | `dependencies_unfinished` | 相依完成後自動派出 |
| `state_unknown` | 帳號還沒核對、無法確認身分 | `profile_unverified`、`identity_unconfirmed` | 在帳號頁核對 |
| `account` | 帳號不可用，或根本沒有帳號 | `no_account`、`profile_not_logged_in`、`auth_source_overridden`、`identity_mismatch`、`duplicate_identity` | 登入、移除覆蓋來源、新增帳號 |
| `guard` | 已啟用的 Guard 擋下（原因帶 `guard`：`guard_id`、`lesson_id`、`lesson_title`） | `guard_violated` | 依來源教訓修改契約，或停用該 Guard（停用後自動派出） |

等待原因每次由目前狀態重新判定，不保存（所以不會過期）。`GET /api/v1/scheduling` 回傳總覽：容量與使用量、每個使用中帳號的進行中 Run（`accounts[].runs`）、額度判斷（`accounts[].quota`、`quota_used`，未知為 `null`）與方案上限（`accounts[].plan_limits`）、佇列與各自的原因。排程只拿到已知用量的帳號（`baton_api::quota::known_used`）。

測試：`crates/baton-harness/tests/scheduling.rs`（情境測試）、`crates/baton-domain/src/scheduler/tests.rs`（單元與性質測試）、`crates/baton-store/src/scheduling/tests.rs`。harness 的 `harness.add_account(name, email)` 建立一個核對通過、可派工的帳號（同一個 email 不能綁兩個 Profile）。

### 額度觀測

規則在 `baton_domain::quota`（純邏輯：最新的觀測 + 現在時間 → 判斷），持久化在 `baton-store` 的 `quota.rs`，輪詢與方案上限端點在 `baton-api` 的 `quota.rs`，事件來源在 `Supervisor`（spec D-12）。

**三個來源**，每筆觀測都存成 `quota_observation`（來源 `source`、`origin`、視窗、狀態、用量比例、`observed_at`、可信度 `confidence`、原始資料參照 `raw_ref` 與原文 `raw`）：

| `origin` | `source` | 從哪裡來 | `raw_ref` |
| --- | --- | --- | --- |
| `adapter` | `observed` | Adapter 解析 CLI 串流的限流訊號：`AdapterEvent::RateLimited { window, status, used, reset_at_millis, raw }`（`status` = `allowed`／`warning`／`exhausted`） | `run:<run_id>` |
| `estimate` | `estimated` | Run 結果訊息的用量 `AdapterEvent::UsageReported { tokens, raw }` 記入 `quota_usage`；滾動五小時與七日視窗內的本機累計 token ÷ 使用者設定的方案上限 | `quota_usage:seq=a..b` |
| `observer` | Port 回報的 | `QuotaObserver` Port 每 5 分鐘輪詢一次（spec D-31）；正式版沒有實作（`NotImplemented` 不留紀錄），其他錯誤記為一筆未知的觀測 | `quota_observer` |

**未知不填零**：沒有方案上限、視窗內沒有本機用量、CLI 訊號沒有數字、Port 失敗，用量都是 `null`、可信度 `unknown`。CLI 訊號在 `reset_at` 之後失效（沒有重置時間時維持一個視窗長度），失效後回到未知。沒有說明視窗的訊號歸到五小時視窗。

**較保守者**（`assess`）：每個視窗取已觀測與推估中用量較高者，帳號再取兩個視窗與外部觀測器中最高者；`exhausted` 視為 100%，`warning` 沒有數字時視為 75%（停止派工門檻）。這個值就是排程用的用量。

**觀測不完整**：推估只看得到本機 Baton 派出的 Run。兩個視窗都有來自 CLI 的帳號層級數字（或外部觀測器有已觀測的數字）才算 `complete`；否則 `incomplete_reason` 說明缺哪個視窗、其他裝置與互動式使用的用量看不到。

**輪詢與推估的時機**：每次記下用量、變更方案上限、輪詢時都重新推估並寫入新的推估觀測。輪詢以 `quota_poll` 記錄每個帳號上次輪詢的時間，Supervisor 每次處理與排程端點都會呼叫 `baton_api::quota::current`，只有到期（≥ 5 分鐘）的帳號才真的呼叫 Port。輪詢時刪除超過 8 天的觀測與用量（每個帳號、來源、視窗最新的一筆保留）。

| 端點 | 用途 |
| --- | --- |
| `PUT /api/v1/profiles/{id}/quota-limits` | 本文 `{ "five_hour_tokens", "seven_day_tokens" }`（正整數 token，或 `null` 表示未設定），回傳新的排程總覽；400 `plan_limit_invalid`（不是正整數）、404 `profile_not_found`、409 `profile_archived`。有變更時追加事件 `quota_plan_limits_changed`（`aggregate_type = 'quota_plan_limit'`） |

帳號頁（`AccountsPage.tsx`）顯示判斷用量、來源、可信度、重置時間與觀測時間、兩個視窗各自的已觀測與推估、觀測不完整的提示與方案上限表單，每 5 分鐘重新讀取一次（`QUOTA_REFRESH_INTERVAL_MS`）。

測試：`crates/baton-harness/tests/quota.rs`（情境測試）、`crates/baton-domain/src/quota/tests.rs`、`crates/baton-store/src/quota/tests.rs`。harness 的 `quota_observer.observe_count(profile)` 回傳輪詢次數。真實 Adapter 的對應（票 14）：`rate_limit_event` 的 `status`（`allowed`／`allowed_warning`／`rejected` → `allowed`／`warning`／`exhausted`）屬於 `rateLimitType` 的視窗，`unifiedWindows` 的其他視窗以 `allowed` 加已用比例另送一個訊號；`tokens` = `result.usage` 的輸入 + 輸出 + 快取建立（快取讀取不計）。**Validation Required**：撞到限制（`rejected`）時的實際形狀尚未實測。

### 成果儲存與成果瀏覽

Worker 產生的成果（Artifact）由 Runtime 保管內容與 manifest。規則在 `baton_domain::artifact`（純邏輯：登記請求驗證、版本遞增、未登記檔案辨識、預覽判定），持久化在 `baton-store`，端點在 `baton-api` 的 `artifacts.rs`（Agent 端 `register_artifact` 與桌面端瀏覽、預覽、下載），檔案存取走 `WorkspaceFs` Port（`write_artifact_atomic`、`read_artifact`、`list_artifact_files`）。

**儲存位置**：`<資料目錄>\artifacts\<workspace_id>\<blob 檔名>`。檔名是 `blob_<隨機>`，**每個版本一個獨立檔案、不用成果名稱當檔名**，所以新版本不會覆寫舊版本。manifest 在控制資料庫的兩張追加式表：

- `artifact`：一個成果（同 Workspace、同 Task、同名 = 同一個成果）。
- `artifact_version`：成果的每個版本，觸發器禁止更新與刪除。欄位：版本、描述、大小、SHA-256、相對於成果儲存根目錄的 `storage_path`（以 `/` 分隔）、產生者 Run 與世代、時間。

**寫入順序**（spec D-17）：

1. 寫暫存檔 `<blob 檔名>.partial`；
2. fsync；
3. 原子改名為 `<blob 檔名>`；
4. 在**單一交易**提交 manifest，並追加事件 `artifact_registered`（`aggregate_type = 'artifact'`，聚合版本 = 成果版本號）。

所以「已登記」一定表示內容完整。Runtime 在中途崩潰只會留下**未登記**檔案，它們不是成果、**不會被自動刪除**，由成果列表的 `unregistered_files` 顯示，恢復時可辨識：

| `kind` | 狀態 | 意義 |
| --- | --- | --- |
| `staging` | 檔名以 `.partial` 結尾 | 寫入尚未完成就中斷，內容不完整，不可使用 |
| `unregistered` | 已改名、沒有任何 manifest 引用 | 內容完整，但 manifest 還沒提交就中斷（或登記被拒）；仍不算成果 |

辨識由 `artifact::classify_stored_files` 完成：成果儲存內沒有被任何 `artifact_version.storage_path` 引用的檔案才會列出。

**Agent 工具 `register_artifact`**（`POST /agent/v1/tools/register_artifact`，只接受 Agent 憑證）：

- 請求 `{ name, description?, content? | content_base64? }`，內容二選一。**內容以參數內嵌傳入**，Runtime 不讀取 Worker 指定的任何路徑，避免 Worker 藉登記讀走工作目錄以外的檔案。
- 請求本文上限 32 MiB（Base64 與 JSON 跳脫會讓本文比內容大）；MCP shim 對 Runtime 的請求逾時是 30 秒。
- 限制：名稱是檔名（不含路徑分隔字元與控制字元，≤ 200 字元，`NAME_MAX_CHARS`）、描述 ≤ 1000 字元（`DESCRIPTION_MAX_CHARS`）、解碼後內容 ≤ 10 MiB（`ARTIFACT_MAX_BYTES`）。
- 成功回 201 `{ artifact_id, version, name, size_bytes, sha256, created }`（`created` 表示是否建立了新成果，false 為既有成果追加新版本）。
- 產生者與 Task 關聯一律取自憑證（Workspace、Run、世代；Task 取自 Run）；請求帶的 `workspace_id`／`run_id` 與憑證不符回 403 `agent_scope_mismatch`。
- 登記**不改變 Task 狀態**；成果是否被接受屬於後續的驗證 Gate（票 13）。

| 錯誤 | 狀態碼 |
| --- | --- |
| `artifact_invalid`（名稱、描述或內容欄位不合規） | 400 |
| `artifact_too_large`（超過 `ARTIFACT_MAX_BYTES`） | 413 |
| `run_report_stale`（Run 已結束或被接替；**不寫入任何檔案與 manifest**） | 409 |
| `agent_scope_mismatch` | 403 |
| `bad_request`（JSON 不合法） | 400 |

**桌面端點**（桌面憑證；Agent 憑證得到 403、無憑證 401，與其他管理 API 相同）都在 `/api/v1/workspaces/{id}/` 之下。成果一律以 Workspace 為界，跨 Workspace 取用回 404：

| 端點 | 用途 |
| --- | --- |
| `GET artifacts` | 成果與全部版本；每個版本帶 `file_status`（`ok`／`missing`／`size_mismatch`，**只比對存在與大小**），另有 `unregistered_files` |
| `GET artifacts/{artifact_id}/versions/{version}/preview` | `{ "preview": {"kind":"text","text","truncated"} \| {"kind":"binary"} }`；文字最多 64 KiB（`PREVIEW_MAX_BYTES`），在字元邊界截斷；含 NUL 或不是有效 UTF-8 的內容是 `binary` |
| `GET artifacts/{artifact_id}/versions/{version}/content` | 下載，原始位元組，標頭 `Content-Disposition: attachment` 與 `X-Baton-Sha256` |

預覽與下載都先**重新計算 SHA-256 與大小**，與 manifest 不符回 409 `artifact_integrity_failed`；檔案不存在回 409 `artifact_file_missing`；不存在的成果或版本回 404 `artifact_not_found`。完整的雜湊核對只發生在預覽與下載，列表的 `file_status` 為了成本只看存在與大小。

Web UI 的「成果與驗證」檢視（`web/src/ArtifactsPage.tsx`，從左側 Workspace 清單進入）列出成果與版本、預覽文字、下載，並顯示未登記檔案。桌面殼透過 Tauri 指令 `artifact_list`、`artifact_preview`、`artifact_download` 呼叫 `RuntimeClient`（`list_artifacts`、`preview_artifact`、`download_artifact`，每次都讀憑證檔並帶標頭）；下載內容在 Tauri 指令的 JSON 回傳值中以 base64 交給 Web UI，Web UI 再核對 `X-Baton-Sha256` 與 manifest 的雜湊。測試：`crates/baton-client/tests/artifacts_client.rs`。

**已知限制**：

- 登記被拒絕（過期世代）不留稽核紀錄，這點與三個回報工具不同。
- 同一個 Run 重送相同內容會產生新版本（不去重）。
- 登記只代表內容完整落地並有 manifest；成果何時被接受見下一節「成果接受與最小完成驗證」。

### 成果接受與最小完成驗證

Worker 宣稱成功或程序正常退出都**不會**讓 Task 完成：Run `succeeded` 只把 Task 推進到 `verifying`。之後只有兩條路能讓 Task 進入 `completed`，規則在 `baton_domain::verification`，持久化在 `baton-store` 的 `verification.rs`，端點在 `baton-api` 的 `verification.rs`，檢查由 Supervisor 經 `baton-runtime` 的 `verifier.rs` 執行：

1. **確定性檢查**：契約的 `verification_checks`（隨契約版本保存，屬「完成與驗證條件」）宣告的每一項都通過。
2. **使用者手動接受**：只接受桌面憑證；Agent 憑證（包括 Worker 自己的）在入口就被拒絕。

**確定性檢查**（JSON 以 `kind` 區分，最多 20 項，儲存契約時驗證，不合規回 400 `contract_invalid`）：

| `kind` | 欄位 | 通過條件 |
| --- | --- | --- |
| `command` | `program`、`args`、`expected_exit_code`（預設 0） | 在受驗 Run 的工作目錄執行（不經過 shell，`CommandRunner` Port），結束碼相符；逾時 10 分鐘、被終止或無法啟動都算未通過 |
| `file` | `path`、`sha256?` | 工作目錄內的相對路徑（不可含 `..`、磁碟代號、從根目錄開始）是存在的檔案，有 `sha256` 時雜湊相符（`WorkspaceFs::read_workspace_file`） |
| `artifact` | `name`、`sha256?` | 這個 Task 有這個名稱的成果，最新版本的檔案與 manifest 雜湊一致，有 `sha256` 時也相符 |

**流程**：Supervisor 每次處理的最後一步（第 8 步）找出 `verifying`、有受驗 Run（世代最大且 `succeeded` 的 Run）而且契約宣告了檢查的 Task，逐項取得事實（命令輸出保留結尾 4000 字元作為證據）、由 Domain 判定，再以**成果接受交易**提交。全部通過 → `completed`；任一未通過 → 回到 `ready`（可修正、可再派工；不會自動重新排隊），證據保存在驗證紀錄。沒有宣告任何檢查的 Task 停在 `verifying`，等使用者手動接受。背景迴圈把這一步放在另一個工作執行，長時間的檢查命令不會擋住啟動、取消與 lease 核對；測試 harness 的 `supervise()` 則會等它做完。Runtime 在檢查途中當掉時沒有任何紀錄，重啟後會重新執行檢查。

**成果接受交易**同時校驗三件事，任一不符就拒絕、什麼都不寫：

| 校驗 | 不符時 |
| --- | --- |
| 受驗 Run（`run_id`＋`generation`）仍是這個 Task 世代最大、已成功結束的 Run | 409 `acceptance_run_stale` |
| 契約版本與檢視時相同，且等於 Run 派工時的契約版本 | 409 `acceptance_contract_stale` |
| 成果 manifest（每個成果的最新版本）與檢視時相同 | 409 `acceptance_manifest_stale` |

契約在 Run 派工後被改版時，Supervisor 不執行檢查，直接記錄一筆「未通過」（原因寫在 `note`），Task 回到 `ready` 重新執行；這個 Run 的成果不能在新契約下被接受，手動接受也會被拒絕。過期 Run 的回報（登記成果、結束事件）本來就因不是目前寫入所有者而被拒絕（`run_report_stale`），所以不能推進 Task。Workspace 封存後，其中 `verifying` 的 Task 不再自動檢查（`automatic` 為 `false`），成果接受（含手動接受）一律 409 `workspace_archived`，Task 停在 `verifying`。

**驗證紀錄**：追加式資料表 `task_verification`（migration 16，觸發器禁止更新與刪除；`run_id` 唯一：一個 Run 最多一筆；部分唯一索引：一個 Task 最多一筆 `passed`）。每筆含方式（`checks`／`manual`）、結果（`passed`／`failed`）、受驗 Run 與世代、契約版本、每項檢查的結果與證據、驗證時的成果 manifest、手動接受者（桌面憑證雜湊的前 16 字元，**不是**憑證本身）與附註。同一個交易追加 Task 事件 `task_verification_recorded`（payload 含上述摘要與未通過的檢查）與 `task_status_changed`（`reason` 為 `verification_passed`／`verification_failed`）。

**桌面端點**（都在 `/api/v1/workspaces/{id}/tasks/{task_id}/` 之下）：

| 端點 | 用途 |
| --- | --- |
| `GET verification` | `VerificationView`：`basis`（接受基礎：`run_id`、`generation`、`contract_version`、`artifacts`；只有 `verifying` 且有受驗 Run 時才有）、宣告的 `checks`、目前的成果 manifest `artifacts`、`automatic`（Runtime 會不會自動檢查）、歷次 `records` |
| `POST verification/accept` | 手動接受，本文 `{ basis, note? }`，`basis` 原樣送回檢視時拿到的值；成功回 200 與最新的 `VerificationView`。Task 不是 `verifying` 回 409 `task_not_verifying`，附註超過 1000 字元回 400 `acceptance_invalid` |

工作面板 `GET tasks` 另回傳 `verifications`（Workspace 全部驗證紀錄）。Web UI 在 `verifying` 的卡片提供「檢視驗證」與「接受成果」，在卡片上顯示最新驗證紀錄（未通過時列出證據）；契約編輯器以每行一項的文字宣告檢查（`command: 程式 引數…`、`command[3]: …`、`file: 路徑 [sha256]`、`artifact: 名稱 [sha256]`）。桌面殼的 Tauri 指令是 `task_verification`、`task_accept`（`RuntimeClient::task_verification`、`accept_task`）。測試：`crates/baton-harness/tests/acceptance.rs`（情境）、`crates/baton-runtime/tests/command_runner.rs`（真實程序）、`crates/baton-client/tests/verification_client.rs`。

**已知限制**：

- 驗證命令以 Runtime 的使用者身分執行，只終止直接啟動的程序（沒有放進 Job Object），命令自己再開的子孫程序在逾時後可能殘留。
- 檔案檢查只擋住路徑字面上的逃逸（`..`、絕對路徑），不處理工作目錄內指向外部的符號連結或 junction。
- 命令輸出以 UTF-8 解讀，非 UTF-8 的主控台輸出（例如 `cmd` 的 Big5）在證據中會變成替代字元。
- 沒有「手動駁回」：沒有宣告檢查而成果不理想時，目前只能取消 Task 或修改契約讓它重新執行。

### 備份與還原演練

控制資料庫與成果的備份（spec D-18）。判斷在 `baton_domain::backup`（觸發、保留、演練核對），檔案與資料庫操作在 `baton_store::backup`，排程在 `baton_runtime::backup::BackupScheduler`，設定頁的狀態端點是 `GET /api/v1/backups`。

**位置**：`<資料目錄>\backups\`。

| 路徑 | 內容 |
| --- | --- |
| `<備份識別>\control.sqlite` | SQLite Online Backup API 的一致快照；`credential` 表已清空並 VACUUM |
| `<備份識別>\manifest.json` | 觸發原因、時間、schema 版本、快照的大小與 SHA-256、快照中登記的全部成果版本（`storage_path`、大小、SHA-256） |
| `blobs\<SHA-256>` | 成果檔案的副本，以內容雜湊命名，多份備份共用 |
| `drill.json` | 最新一次還原演練的結果 |

備份識別是 `<13 位數毫秒>-<觸發原因>`。備份先寫在 `<備份識別>.staging`，manifest 寫完才改名，所以列得出來的都是完整的備份。

**觸發**：

- 每日：距離上一份備份（不論原因）滿 24 小時。
- 重要裁定：每次正式裁定（`POST …/decisions/{id}/answer`）都要求一次；距離上一份備份滿一小時才執行，同一小時內的要求合併成一份。Stage 完成也呼叫 `BackupScheduler::request_milestone_backup`（Stage 在第四階段才有）。
- migration 前：`Store::open` 發現既有資料庫需要套用 migration 時先備份；**備份失敗就不套用 migration、Runtime 不啟動**（`StoreError::PreMigrationBackup`）。

背景迴圈每分鐘處理一次；harness 關閉背景迴圈（`RuntimeOptions::background_backup = false`），由測試呼叫 `Harness::run_backups` 與 `Harness::request_milestone_backup`。

**保留**（每次備份後套用）：最近 7 個有備份的 UTC 日子各留最新一份、最近 4 個有備份的週（星期一開始）各留最新一份，另外永遠留下最新一份 migration 前的備份；其餘刪除，沒有被任何保留備份引用的成果副本一併刪除。同一天的多份備份只會留下最新的那份。

**還原**：`baton-runtime restore <備份目錄> <目標資料目錄>`（或 `baton_store::backup::restore_backup`）。目標必須是空目錄或不存在。還原會核對快照雜湊、`integrity_check`、`foreign_key_check`、每個成果檔案的大小與雜湊、資料庫登記的成果版本與 manifest 一致；資料庫完整時接著套用 migration，並**結束備份當下尚未結束的 Run**：已啟動的標為 `interrupted`（`end_reason` 以 `restored_from_backup` 開頭）、還沒啟動的取消，Task 回到 `ready`。所以在還原的目錄上啟動 Runtime 不會重新啟動任何程序或接上舊的事件串流；舊的 Agent 憑證也已無效。派工佇列（`dispatch_request`）保留，Scheduler 會照常派出排隊中的 Task。

**還原演練**：每週一次（第一次在第一份備份之後），把最新的備份還原到 `backups\.drill\`、核對上述各項，結果寫進 `drill.json`，隔離目錄用完即刪。設定頁（`web/src/BackupSettings.tsx`，Tauri 指令 `backup_status`、`RuntimeClient::backups`）顯示備份清單與最新的演練結果。

**不在備份內**：Profile 的原生設定目錄（登入資料）、桌面憑證檔、Run 設定快照、日誌。測試：`crates/baton-harness/tests/backup_and_restore.rs`（含逐位元組的憑證掃描）、`crates/baton-store/src/backup/tests.rs`。

**已知限制**：

- 重要裁定的待處理要求只存在記憶體，Runtime 重啟時遺失，最遲由下一次每日備份涵蓋。
- 備份失敗只記日誌（`備份：…`），設定頁看得到的是「最新一份備份的時間」與演練結果。
- 異地副本（使用者指定的額外備份目錄）尚未實作。

### 桌面生命週期與完全退出

規則在 `baton_domain::lifecycle`（純邏輯），持久化在 `baton-store` 的 `lifecycle.rs`（migration 18：`runtime_lifecycle`、`workspace_lifecycle`），端點在 `baton-api` 的 `lifecycle.rs`，停止流程與重新開啟在 `Supervisor`（`full_exit`、第一次 `tick` 的重新開啟）。對應 ADR-0001（DR-20261021-01）與 spec D-11、D-25。

**關窗不等於停止**：Runtime 是獨立程序。桌面殼關閉主視窗只會隱藏到系統匣；系統匣有狀態（每 3 秒更新）、「開啟 Baton」與「完全退出」。重開視窗以 `ensure_runtime` 連回同一個 Runtime，不會啟動第二個控制平面，也不會重複啟動 Worker。

**完全退出**（系統匣 → Web UI 的 `ExitProgress` → `POST /api/v1/lifecycle/exit`）：

1. API 在一個交易中持久化停止意圖（`phase = stopping`，期限 = 要求時間 + 40 秒），並暫停每個使用中的 Workspace；接著把停止意圖鏡像到 `<資料目錄>\lifecycle.json`（暫存檔 + fsync + 改名），回應 202。重複要求是冪等的。
2. 停止意圖生效後，所有會建立新 Run 的路徑都被擋住：Scheduler 的 `Capacity.halt`（佇列重試、額度恢復後的派工、交接都經過 `scheduler::place`），以及寫入 launch intent（`BeginLaunch`）的交易（`launch_halted`，代碼 `launch_halted`）。被擋住的 Task 留在佇列，等待原因是第七類 `lifecycle`（`runtime_stopping`；Workspace 暫停時是 `awaiting_resume`）。
3. 停止流程（Runtime 背景）：再鏡像一次生命週期檔（API 寫入失敗時補上），等 Supervisor 的 tick 鎖（正在停止已取消 Run 的處理看到完全退出就讓出），再對每個尚未結束的 Run 呼叫 `ProcessControl::stop(identity, 30 秒)`（ProcessControl 在寬限期後以 Job Object 終止程序樹，並在 10 秒內確認），全部並行。總上限 40 秒從持久化的期限算起，等鎖的時間也算在內；到期限仍拿不到鎖時不要求任何程序停止，全部記為 `stop_unconfirmed`。拿到鎖後握到結束，不會啟動任何程序。Worker 透過 Agent 回報的回應 `stop_requested: true` 得知要保存並停止。
4. 程序確認停止（或根本沒有程序）的 Run 以 `interrupted`（`stopped_by_full_exit`）結束、釋放寫入所有權，Task 退回 `ready` 並重新排入派工佇列；使用者先前已要求取消的 Run 則完成取消。`StillAlive`、停止呼叫失敗、到總上限仍沒有結果的記為**停止異常**（`still_alive`／`stop_failed`／`stop_unconfirmed`），Run 保留寫入所有權。完全退出不會把任何 Task 標為取消或完成。
5. 進度（每個 Run 的結果）寫在 `runtime_lifecycle.last_exit`，結束時進入 `stopped` 並追加 `full_exit_completed`（含異常清單）。Runtime 再提供 API 5 秒（讓 App 讀最終結果）後結束。App 沒有停止異常就自動結束；有異常則列出並等使用者按「關閉 Baton」。

**重新開啟**：Runtime 啟動時讀資料庫與生命週期檔，不一致時採較保守者（只有檔案有停止意圖或檔案毀損都視為停止，並補記進資料庫）。停止意圖仍生效時，Supervisor 第一次 `tick` 先核對（以 command_id 找回啟動中的程序、查證上次沒能確認停止的程序：確定已不在才以 `process_gone_after_full_exit` 中斷，不自動重派），再解除停止意圖：開啟「完全退出後自動續跑」的 Workspace 解除暫停，排隊中的工作照常派出；其餘 Workspace 等使用者按「繼續工作」。啟動後若又有新的退出要求（生命週期版本改變），不會重新開啟。

**各 Workspace 的設定**：「完全退出後自動續跑」預設關閉；變更只影響下次重新開啟，不會立即啟動或停止目前的工作。

| 端點（桌面憑證） | 用途 |
| --- | --- |
| `GET /api/v1/lifecycle` | 階段（`running`／`stopping`／`stopped`）、`stop_intent_active`、時間參數、進行中或最近一次的停止報告（`exit`）、進行中 Run 數、各 Workspace 的 `auto_resume_after_exit`／`paused_since`／`awaiting_resume` |
| `POST /api/v1/lifecycle/exit` | 完全退出：202 與目前狀態 |
| `PUT /api/v1/workspaces/{id}/lifecycle` | `{ "auto_resume_after_exit": bool }`；找不到 Workspace 404 |
| `POST /api/v1/workspaces/{id}/resume` | 「繼續工作」；停止意圖仍生效時 409 `stop_intent_active` |

事件：`runtime_lifecycle` 聚合有 `full_exit_requested`、`full_exit_completed`、`runtime_reopened`；`workspace_lifecycle` 聚合有 `workspace_paused_by_exit`、`workspace_resumed`、`workspace_auto_resume_changed`。

**Validation Required**：30 秒寬限 + 10 秒確認在真實 Claude Code 與 Windows Job Object 上是否足夠；系統匣、關窗隱藏與停止進度畫面只有編譯與元件測試，需在 Windows 上手動確認（按「完全退出」→ 進度 → App 結束、資料目錄出現 `lifecycle.json`）。

### 知識、教訓與全文檢索

決策、教訓與一般知識都是 Knowledge Entry（spec D-26、D-28）。規則在 `baton_domain::knowledge`（純邏輯：內容驗證、狀態轉換、版本、取代關係、可見範圍、搜尋關鍵字解析），持久化與搜尋在 `baton-store` 的 `knowledge.rs`，端點在 `baton-api` 的 `knowledge.rs`（管理 API 與 Agent 工具 `record_lesson_candidate`、`search_knowledge`）。

**資料模型**（migration 17 `knowledge`）：

- `knowledge_entry`：一張表放全部條目，以 `kind` 區分 `decision`（決策）、`lesson`（教訓）、`fact`（一般知識）。保存範圍、可信狀態、來源、有效期間（`valid_from`、`valid_until`）、目前內容的版本號 `version`、聚合版本 `revision`（每次變更都遞增，操作時用來做 expected_revision 校驗）、取代關係（`supersedes`、`superseded_by`）、撤銷或淘汰的原因，以及目前內容的檢索文字 `search_text`。觸發器禁止刪除：**條目不可刪除**，撤銷與淘汰都只改變狀態。
- `knowledge_entry_version`：內容（標題、內文、標籤、證據；教訓另有觸發條件 `trigger`、根因 `root_cause`、解法 `resolution`）的每個版本，追加式（觸發器禁止更新與刪除）。修正只追加新版本，舊版本內容保留。
- 每次變更在單一交易中寫回條目、追加版本與事件（`aggregate_type = 'knowledge_entry'`，聚合版本 = `revision`）：`knowledge_created`、`knowledge_confirmed`、`knowledge_corrected`、`knowledge_revoked`、`knowledge_retired`、`knowledge_superseded`。條目詳情的變更歷史就是這些事件。

**可信狀態與轉換**。狀態由來源決定，呼叫者不能指定：使用者在「知識」分頁建立的條目直接是**已驗證**；Agent 只能提交教訓，而且一律是**候選**，必須由使用者確認才升為已驗證（D-28）。

| 操作 | 允許的目前狀態 | 結果 |
| --- | --- | --- |
| 確認 | `candidate`（候選） | → `verified`（已驗證） |
| 修正 | `candidate`、`verified` | 狀態不變（候選仍是候選）；內容有變才追加新版本、`version` 加 1；只改有效期限不追加版本，但仍留下 `knowledge_corrected` 事件；內容與期限都沒變更回 400 `knowledge_invalid` |
| 撤銷（內容是錯的） | `candidate`、`verified` | → `revoked`（已撤銷）；必須說明原因 |
| 淘汰（曾經正確、已不適用） | `verified` | → `retired`（已淘汰）；必須說明原因 |
| 取代（建立新條目時指定 `supersedes`） | 被取代者須為 `verified`、同種類 | 新條目為已驗證；舊條目在同一個交易中 → `superseded`（已被取代），兩者以 `supersedes`／`superseded_by` 互相連結。只有使用者可以指定 |

已撤銷、已淘汰、已被取代之後不能再做任何操作。所有操作都帶 `expected_revision`，與目前不符回 409 `knowledge_revision_conflict`（同時編輯不會互相覆寫）；目前狀態不允許回 409 `knowledge_transition_invalid`。

**有效**：已驗證，而且現在在有效期間內（`valid_from` ≤ 現在 < `valid_until`，沒有 `valid_until` 表示沒有期限；`valid_from` 省略時從建立時間起）。候選、撤銷、淘汰、被取代與過期的條目都**不是有效的**，不會被當作有效知識載入或被 Agent 搜尋到。API 回傳的每個條目都帶 `effective` 表示現在是否有效。

**範圍**：全域、Workspace、Goal、Task。Task 範圍的 Goal 一律由 Runtime 依 Task 補上；範圍內的 Workspace、Goal、Task 必須存在且彼此一致，否則 400 `knowledge_scope_invalid`。誰看得到哪些條目：

| 誰 | 看得到 |
| --- | --- |
| 桌面（知識分頁，帶 `workspace_id`） | 全域條目，加上該 Workspace 內全部範圍（Workspace、所有 Goal、所有 Task）的條目 |
| 桌面（不帶 `workspace_id`） | 只有全域條目 |
| Agent Run | 全域，加上它的 Workspace 範圍、它的 Goal 範圍、它的 Task 範圍的條目；其他 Goal 或 Task 的條目看不到 |

**管理 API**（桌面憑證；Agent 憑證得到 403 `role_not_allowed`）：

| 端點 | 用途 |
| --- | --- |
| `GET /api/v1/knowledge?workspace_id=&q=&include_inactive=` | 列表與搜尋，回傳 `{ search_mode, entries }`。`include_inactive=false`（預設）只回傳有效條目，`true` 時連同候選、撤銷、淘汰、被取代與過期的一起回傳；`workspace_id` 不存在回 404 `workspace_not_found` |
| `POST /api/v1/knowledge` | 建立 `{ kind, scope: { kind, workspace_id?, goal_id?, task_id? }, title, body?, tags?, evidence?, lesson?, valid_from?, valid_until?, supersedes? }`，回 201（直接是已驗證） |
| `GET /api/v1/knowledge/{id}` | 詳情 `{ entry, versions, history }`：目前狀態、全部內容版本（由舊到新）與變更歷史（由舊到新） |
| `POST /api/v1/knowledge/{id}/confirm` | `{ expected_revision }`：候選 → 已驗證 |
| `POST /api/v1/knowledge/{id}/correct` | `{ expected_revision, title, body?, tags?, evidence?, lesson?, valid_until?, note? }`：以完整的新內容修正；`valid_until` 為空表示沒有期限 |
| `POST /api/v1/knowledge/{id}/revoke` | `{ expected_revision, reason }`：撤銷 |
| `POST /api/v1/knowledge/{id}/retire` | `{ expected_revision, reason }`：淘汰 |

**Agent 工具**（`POST /agent/v1/tools/…`，只接受 Agent 憑證，經 MCP shim 呼叫；身分取自憑證，請求帶的 `workspace_id`／`run_id` 不符回 403 `agent_scope_mismatch`）：

- `record_lesson_candidate`：提交候選教訓。`scope` 是 `task`（預設）、`goal` 或 `workspace`，一律以憑證所屬 Run 的 Task 解析，**不接受識別**；`global` 回 400 `knowledge_scope_invalid`。來源記錄提交的 Run、世代與 Task。Run 已結束或被接替回 409 `run_report_stale`，什麼都不寫。
- `search_knowledge`：`{ query? }`，只回傳這個 Run 看得到且**有效**的條目（`{ search_mode, entries }`）；空白 `query` 列出全部有效條目。Run 已結束或被接替回 401 `credential_revoked`。

錯誤代碼（`KnowledgeError::code`）：`knowledge_invalid`、`knowledge_query_invalid`、`knowledge_scope_invalid`（400）、`knowledge_not_found`（404）、`knowledge_revision_conflict`、`knowledge_transition_invalid`、`knowledge_supersede_invalid`（409）。內容上限（標題、內文、教訓欄位、標籤、證據、原因的字元數與項數）見 `baton_domain::knowledge` 的常數。

**全文檢索**（spec D-26）：

- 檢索文字 `search_text` 是目前版本的標題、內文、教訓三欄、標籤與證據，以換行連接。
- 索引是 FTS5 虛擬表 `knowledge_fts`（`tokenize = 'trigram'`，支援繁體中文子字串）。它是**可重建的衍生資料**，不在 migration 中：Store 每次開啟時建立（若不存在）、清空並從 `knowledge_entry.search_text` 整個重建；之後每次寫入條目時在同一個交易更新索引。
- 關鍵字以空白分詞，**全部都要出現**（AND）；每個詞至少 2 個字元（`QUERY_TERM_MIN_CHARS`），最多 8 個詞、整個查詢最多 200 字元，不合規回 400 `knowledge_query_invalid`。3 個字元以上的詞以片語 `MATCH` 查 trigram 索引；2 個字元的詞 trigram 無法涵蓋，改以 `LIKE` 逐筆比對 `search_text`。結果依最後更新時間由新到舊，最多 100 筆（`RESULT_LIMIT`）。
- **降級**：FTS5 或 trigram tokenizer 不可用（開啟時建立索引失敗）時，全部關鍵字都退回 `LIKE`，結果相同但較慢。列表與 `search_knowledge` 的回應以 `search_mode` 標示（`full_text` 或 `like_fallback`），Runtime 啟動時在日誌記錄降級（「全文檢索（FTS5 + trigram）無法使用，知識搜尋已降級為逐筆 LIKE 比對」），Web UI 顯示降級提示。因為索引每次啟動都從條目重建，降級期間寫入的條目在 FTS5 恢復後一樣搜得到。
- 開關：`StoreOptions { full_text_search }`（`Store::open_with`）與 `RuntimeOptions.full_text_search`，預設都是 `true`；設為 `false` 視同 FTS5 不可用。bundled SQLite 一定有 FTS5，所以這是重現降級的唯一方式，正式執行不會關閉它。

Web UI 的「知識」分頁（`web/src/KnowledgePage.tsx`，從左側 Workspace 清單的「知識」進入）：搜尋、待確認的候選教訓、有效條目與歷史條目（撤銷、淘汰、被取代、過期）分開列出；可新增條目（範圍只提供全域或此 Workspace）、確認、修正、撤銷、淘汰，並在詳情查看全部版本與變更歷史；降級時顯示提示。桌面殼透過 Tauri 指令 `knowledge_list`、`knowledge_create`、`knowledge_detail`、`knowledge_confirm`、`knowledge_correct`、`knowledge_revoke`、`knowledge_retire` 呼叫 `RuntimeClient`。

測試：`crates/baton-harness/tests/knowledge.rs`（情境測試，降級情境見〈測試 harness〉的 `without_full_text_search`）、`crates/baton-domain/src/knowledge/tests.rs`、`crates/baton-store/src/knowledge/tests.rs`。

**已知限制**：

- Run 啟動時把有效知識載入 context 與 Context Receipt 不在本票範圍；目前 Agent 只能主動以 `search_knowledge` 取得。
- 由教訓轉成 Guard 見〈Guard 與執行前檢查〉（票 38）。
- 修正不能改變種類、範圍或 `valid_from`；要改這些請建立新條目並取代舊條目。

### Guard 與執行前檢查

已驗證的教訓可以由使用者轉成 **Guard**，在 Run 准入（派工交易）時由程式檢查，而不只是提醒（spec 使用者故事 60、D-28）。規則在 `baton_domain::guard`，持久化在 `baton-store` 的 `guards.rs`，端點在 `baton-api` 的 `guards.rs`，UI 在 `web/src/KnowledgePage.tsx` 的「Guard」區塊與工作面板的排隊原因。

**規則**（`GuardRule`，JSON 以 `kind` 區分）：

| `kind` | 參數 | 違反條件 |
| --- | --- | --- |
| `preflight`（啟動前檢查） | `field`（六項契約欄位之一）、`must_mention`（1–200 字元） | 該欄位不是「已填寫」，或內容沒有提到 `must_mention`（不分大小寫） |
| `tool_restriction`（工具限制） | `capability` | Task 的 `required_capabilities` 含這項能力 |
| `retry_policy`（重試政策） | `max_failed_runs`（1–20） | 同一個 Task 在**目前契約版本**下 `failed` 的 Run 數 ≥ 上限（改契約即重新計算；`interrupted`、`cancelled` 不算） |

**誰能建立、何時生效**：只有桌面憑證能建立、啟用、停用（Agent 憑證在入口防護 `guard` 中介層就得到 403 `role_not_allowed`）。只有**有效**的教訓（`kind = lesson`、已驗證、在有效期間內）能轉成 Guard，否則 409 `lesson_ineligible`；使用者建立即啟用。Guard **生效** = 啟用 + 來源教訓現在仍有效：教訓被撤銷、淘汰、取代或過期後，Guard 自動不再檢查（列表的 `active = false`、`inactive_reason` 說明原因），也不能再啟用。Guard 不能刪除，只能停用。啟用、停用帶 `expected_revision`，不符回 409 `guard_revision_conflict`。

**檢查時機與範圍**：`run::check_dispatch` 在授權准入之後、相依之前呼叫 `guard::violations`。Store 在派工路徑（`Store::schedule` 的新要求與佇列重試、`scheduling_snapshot`、`apply_run_command` 的 `Dispatch`）以 `load_active_guards` 載入生效中的 Guard 放進 `RunContext.guards`；其他路徑是空的。只檢查來源教訓範圍涵蓋的 Task（與 Agent 看得到的範圍相同：全域、同 Workspace、同 Goal、同 Task）。pending → ready **不**檢查 Guard（重試政策依賴 Run 紀錄，只在 Run 准入時有意義）。

**擋下時**：不建立 Run、不啟動程序，Task 留在 `ready` 並留在派工佇列。派工回 202，`reasons` 中每個違反的 Guard 各一個 `category = guard`、`code = guard_violated` 的原因，訊息含違反說明與來源教訓標題，`guard` 欄位帶 `guard_id`、`lesson_id`、`lesson_title`；`GET /api/v1/scheduling` 的佇列與工作面板的卡片顯示同樣的原因與「來源教訓」。修改契約（Task 退回 pending、離開佇列）或停用 Guard（喚醒 Supervisor，佇列重試時自動派出）即可解除。

**重犯事件**（`lesson_recurrence`，追加式）：教訓警告過的情況又出現一次，一律指向來源教訓。

- `prevented`：Guard 在 Run 准入時擋下，`Store::schedule` 在同一個交易中記錄（指向 Guard、Task 與契約版本）。識別由「Guard + Task + 契約版本」決定，佇列每秒重試看到同一個違反只記一次；改了契約仍違反就是新的一次。
- `reported`：沒被攔下、由使用者在知識分頁回報（說明必填，可指定 Task）。
- 每筆同時追加事件 `lesson_recurrence_recorded`（`aggregate_type = 'lesson_recurrence'`，payload 含 `lesson_id`、`guard_id`、`task_id`、`kind`），可在事件時間線追溯。

**成效**（Guard 列表的每個 Guard）：`prevented_count`、`last_prevented_at`（被它攔下的次數與最近時間），以及 `reported_count`（Guard 建立之後，同一個教訓沒被攔下、由使用者回報的次數）。

桌面憑證專用的端點：

| 端點 | 用途 |
| --- | --- |
| `GET /api/v1/guards?workspace_id=` | `{ guards }`：來源教訓是全域或屬於該 Workspace 的 Guard（省略 `workspace_id` 只列全域），每個附來源教訓（標題、狀態、`effective`、範圍）、`active`、`inactive_reason`、`description`（規則說明）與成效 |
| `POST /api/v1/guards` | `{ lesson_id, rule }`，回 201（啟用）。400 `guard_invalid`、404 `knowledge_not_found`、409 `lesson_ineligible` |
| `POST /api/v1/guards/{id}/enable`、`…/disable` | `{ expected_revision }`；已是目標狀態時不變更。404 `guard_not_found`、409 `guard_revision_conflict`、409 `lesson_ineligible`（啟用時教訓已不有效） |
| `GET /api/v1/knowledge/{id}/recurrences` | `{ recurrences }`，新到舊 |
| `POST /api/v1/knowledge/{id}/recurrences` | `{ note, workspace_id?, task_id? }` 回報沒被攔下的重犯，回 201。教訓必須是已驗證的教訓（過期也可以），否則 409 `lesson_ineligible`；說明空白或 Task 不存在 400 `guard_invalid` |

資料表（migration 20 `guard`）：`guard`（觸發器禁止刪除）、`lesson_recurrence`（追加式，觸發器禁止更新與刪除；`prevented` 必定有 `guard_id`）。Guard 的事件：`guard_created`、`guard_enabled`、`guard_disabled`（`aggregate_type = 'guard'`，聚合版本 = `revision`）。桌面殼的 Tauri 指令：`guard_list`、`guard_create`、`guard_enable`、`guard_disable`、`knowledge_recurrences`、`knowledge_report_recurrence`。

測試：`crates/baton-harness/tests/guards.rs`（情境測試）、`crates/baton-domain/src/guard/tests.rs`、`crates/baton-store/src/guards/tests.rs`、`crates/baton-client/tests/guards_client.rs`。

**已知限制**：Guard 只在 Run 准入時檢查，不攔截執行中的工具呼叫（權限提示仍依 Workspace 授權判定）；Context Builder 把教訓載入 Prompt 屬票 39。

### Worktree、輸出目錄與整合佇列

Task 有**種類**（`kind`：`development`／`research`／`document`，建立時決定、預設 `research`）。規則在 `baton_domain::workdir` 與 `baton_domain::integration`，持久化在 `baton-store` 的 `workdirs.rs`、`integration.rs`（migration 21：`task.kind`、`task_workdir`、`integration_entry`），準備目錄在 `baton-api` 的 `workdirs.rs`，整合由 Supervisor 執行。

**工作目錄**：開發 Task 在 `<Workspace>\.baton\worktrees\<Task>`（從 Workspace 目前簽出的分支建立平台分支 `baton/<Task>`，該分支即整合目標），研究與文件 Task 在 `<Workspace>\.baton\outputs\<Task>`。放在 Workspace 目錄內，是因為 Workspace 授權的目標就是 Workspace 目錄；`.baton\.gitignore`（內容 `*`）讓主 checkout 不出現未追蹤的檔案。

- **派工之前準備**：使用者要求派工（`POST …/dispatch`）與 Supervisor 排程前，對排隊中的 `ready` Task 在交易外建立目錄（`WorkspaceFs::inspect_git`、`create_worktree`、`ensure_directory`），成功或失敗都登記到 `task_workdir`（事件 `task_workdir_prepared`／`task_workdir_failed`）。派工（`run::check_dispatch`）只認已登記且可用的目錄；不可用時不派工，排隊原因的類別是 `workdir`（`workdir_not_prepared`、`workdir_unavailable`，訊息就是登記的失敗原因，例如「這個 Workspace 不是 Git repo」）。失敗後排程每 60 秒重試一次；使用者再次要求派工時立即重試。建立是冪等的：建好之後、登記之前崩潰，下次準備會沿用已存在的 worktree。
- **一個 Task 一個目錄**：同一個 Task 的後續 Run 回到同一個目錄（原生 Session 以工作目錄區分專案）；不同 Task 的目錄一定不同（可用的路徑有唯一索引）。工作面板 `GET tasks` 回傳 `workdirs`（每個附 `run_ids`：派工時固定在啟動資訊中、工作目錄相同的 Run）。
- 確定性檢查的命令與檔案檢查在 Run 的工作目錄中執行（研究 Task 是輸出目錄，不是 Workspace 目錄）。

**整合佇列**（開發 Task）：成果接受交易（確定性檢查全部通過或手動接受）在同一個交易中把平台分支排入 `integration_entry`，Task 停在 `verifying`（`VerificationView.basis` 變成 `null`，不能再接受一次：409 `acceptance_already_queued`）。Supervisor 在驗證之後依序處理，一次一個（處理中的項目有唯一索引保證最多一個）：

1. `merging`：先持久化，再在整合目錄 `<Workspace>\.baton\integration` 以 detached HEAD 簽出目標分支並 `--no-ff` 合併（`WorkspaceFs::merge_for_integration`）；目標分支本身不動。衝突 → `failed`。
2. `verifying`：以 Task 契約的同一份確定性檢查，在整合目錄中執行整合驗證。任一未通過 → `failed`，證據保存在項目的 `checks`。
3. `advancing`：把目標分支從合併當時的 commit 以 fast-forward 前移到合併結果（`advance_branch`，冪等）；整合期間目標分支被更新 → `failed`。
4. `integrated`：Task 進入 `completed`。

`failed` 時 Task 回到 `ready`（可修正、可重新派工；重新派工的 Run 回到同一個 worktree），目標分支不受影響。Runtime 在 `merging`／`verifying` 時崩潰，重啟後從合併重做；在 `advancing` 時崩潰，以同一個合併結果重試前移。契約沒有宣告檢查（手動接受）時，乾淨的合併就是整合的證據。端點：`GET /api/v1/integration-queue`（全域佇列：`active` 與依排入順序的 `entries`）；工作面板另回傳 `integrations`，`GET …/verification` 另回傳最新的 `integration`。

**正式版 Git 操作**（`baton_ports::git_*`，以 `git -C <目錄>` 執行、不經過 shell）：整合目錄是主 checkout 或簽出分支的 worktree 時拒絕重設；前移時，若有工作目錄簽出目標分支就在那裡 `merge --ff-only`（有未提交的變更擋住時回報「無法前移」），否則以 `update-ref` 比對舊值後前移。只在 `user.name`／`user.email` 沒有設定時才以 `Baton <baton@localhost>` 作為合併 commit 的作者。測試：`crates/baton-ports/src/git.rs`（真實 git）、`crates/baton-harness/tests/worktrees.rs`（情境）。

**已知限制**：worktree 的移除與未保存變更檢查屬於清理（票 21），目前 `remove_worktree`、`inspect_dirty` 的正式版仍是「尚未實作」。權限提示的路徑判定仍以 Workspace 目錄為基準（相對路徑以 Workspace 目錄解讀，不是 Run 的工作目錄）。整合未通過後不會自動重新排入佇列。

### 帳號 Profile 與首次啟動精靈

每個 account_profile 有自己的 `CLAUDE_CONFIG_DIR`：`<資料目錄>\profiles\<profile id>`（由 Runtime 建立，識別是隨機值；任兩個 Profile 的目錄不相同也不互相包含，已封存的 Profile 目錄不會被重用）。資料庫只保存核對結果（登入與否、認證來源、帳號身分、核對時間），登入資料（token）只存在該目錄中、由 Claude Code 保管，不進資料庫、日誌、事件、API 回應或備份。最多同時 8 個使用中的 Profile（spec D-32）；封存會釋出名額但不刪除目錄。

- `GET /api/v1/profiles`：全部 Profile（含已封存）、`limit`，每個 Profile 帶 `dispatchable` 與 `blockers`（`{code, message}`）。**Scheduler 之後只能派給 `dispatchable` 的 Profile。**
- `POST /api/v1/profiles`（`{ name, expected_account? }`）：第 9 個回 409 `profile_limit_reached` 並說明上限；被拒絕時不會留下目錄。
- `POST /api/v1/profiles/{id}/verify`：以該目錄執行 `claude auth status` 核對。實際認證來源不是該 Profile 的原生登入（`ANTHROPIC_API_KEY`、`apiKeyHelper`、`CLAUDE_CODE_OAUTH_TOKEN`、第三方供應商等）、帳號與預期身分不符、設定目錄不符、同一帳號已被較早的 Profile 使用，都會標示並阻擋該 Profile 被派工。沒有預期帳號時，第一次在原生登入核對到的帳號會固定為預期身分。Claude Code 沒有回應時回 502 `profile_check_failed`，不改寫既有結果。
- `POST /api/v1/profiles/{id}/login-terminal`：開啟終端機，以該 Profile 的 `CLAUDE_CONFIG_DIR` 執行 `claude auth login`（`cmd /c start … cmd /k claude auth login`）。登入完成後由使用者按「重新核對」。
- `POST /api/v1/profiles/{id}/archive`。
- `GET /api/v1/setup`：精靈狀態（Claude Code 是否在 PATH 與版本、Git、使用中 Profile 的登入狀態、`ready`、背景行為告知）；`POST /api/v1/setup/recheck`（「重新檢查」，另外重新核對每個使用中的 Profile）；`POST /api/v1/setup/background-notice/acknowledge`。
- 缺項只回傳官方指令供顯示（`guidance`），Runtime 不下載、不執行任何安裝腳本；Port 上也沒有安裝動作。最低 Claude Code 版本是 Adapter 契約驗證過的 2.1.307（`baton_domain::claude_code::MIN_SUPPORTED_VERSION`，精靈的 `MIN_CLAUDE_CODE_VERSION` 與它相同）。核對 Profile 時同時記下 `claude --version`（`verification.cli_version`）：版本低於最低版本回報 `cli_version_unsupported`，找不到 Claude Code 或看不懂版本回報 `cli_version_unknown`，兩者都阻擋該 Profile 的派工（票 14 之前的核對沒有版本，需要重新核對）。

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
| `settings_snapshot_failed` | 500 | 無法寫入或讀取快照目錄 |
| `settings_snapshot_mismatch` | 409 | 已綁定的快照目錄與版本內容或 `manifest.json` 不一致（被修改、刪除或多出檔案） |

**版本不可變**：版本存在追加式的 `shared_settings_version` 表（版本號從 1 連續遞增；觸發器禁止更新與刪除），採用時在同一交易追加 `shared_settings_adopted` 事件（`aggregate_type = 'shared_settings'`，聚合版本 = 版本號）。**每次採用都建立新版本**，即使內容與目前版本相同（預覽的 `changed` 會告知這一點）；舊版本內容永遠不變。**採用必須帶預覽的 `candidate_hash`**：使用者採用的一定是他看過的內容，預覽之後來源檔有任何改變，採用就被拒絕。

**Run 設定快照**：`settings-snapshot` 把版本渲染成 `<資料目錄>\run-settings\<run_id>\`，只包含有內容的類別檔案，外加清單：

| 檔案 | 內容 |
| --- | --- |
| `settings.json` | 偏好、hooks、plugins 與市集 |
| `mcp.json` | `{ "mcpServers": … }` |
| `CLAUDE.md` | 全域指示 |
| `manifest.json` | 版本、內容雜湊與上列每個檔案的 SHA-256（供核對） |

渲染完全由版本內容決定（鍵排序、固定縮排），同一版本永遠得到相同的位元組；`bundle_from_snapshot_files` 可從目錄還原內容核對版本雜湊。寫入由 `WorkspaceFs::write_readonly_files` 完成：先寫到同層的 `<run_id>.staging`、每個檔案 `fsync` 並設為唯讀屬性，才改名成正式目錄，最後把目錄也設為唯讀，所以讀取端不會看到寫到一半的目錄。Run 與版本的綁定存在追加式的 `run_settings_snapshot` 表（觸發器禁止更新與刪除）：**Run 第一次產生快照時綁定版本，之後再採用新版本也不影響它**；再次呼叫回傳同一個綁定，要求別的版本回 409 `settings_snapshot_pinned`。

**准入與核對（票 14）**：快照由 `baton_api::RunSettings` 產生，API 端點與 Supervisor 共用同一個實例：**同一個 Run 的請求依序處理**（不會兩個請求同時寫同一個目錄）；**已綁定的快照每次都核對**：目錄中的檔案必須與版本渲染出的檔案（含 `manifest.json`）完全相同，不一致回 409 `settings_snapshot_mismatch`。Supervisor 啟動 Run 前自動取得快照：尚未綁定就以最新版本建立，已綁定就核對；核對不過 Run 以 `launch_failed：settings_snapshot_mismatch…` 結束，**不啟動程序**。還沒有採用任何版本時 Run 不帶共同設定啟動。

> **Validation Required**（spec D-09）：
>
> - 快照目錄在 Windows 上只靠**檔案的唯讀屬性**保護：它能阻止直接覆寫檔案（情境測試驗證），但**不阻止刪除檔案**（實測 Rust 標準函式庫的刪除會忽略唯讀屬性，Node 的 `unlink` 也會先取消它）、**目錄本身無法阻止新增檔案**，唯讀屬性也不是安全邊界（同一 Windows 使用者可以取消它）。快照檔的內容雜湊記在 `manifest.json`；Run 啟動前會核對（見上），但啟動之後被改動不會被偵測（AI-114）。
> - Claude Code 是否能完全阻擋使用者層設定（`~\.claude\settings.json` 等）的合併與熱重載，**尚未實測**；在實測之前不可宣稱 Run 的設定已被固定。無法維持時，依 D-09 必須阻擋並顯示，不宣稱固定成功。
> - Adapter 以 `--settings`、`--append-system-prompt-file` 與 `--mcp-config` 把快照檔傳給 Claude Code，並以 `--setting-sources ""` 不載入任何設定檔來源（使用者層與 Workspace 內的 `.claude\settings*.json`，實測見 `docs/adapter-contract.md`）；使用者層 `CLAUDE.md` 是否仍被載入、熱重載是否受影響仍待實測。

### Claude Code Adapter

正式的 AgentAdapter 是 `baton_runtime::claude_adapter::ClaudeCodeAdapter`（spec D-06、D-07）：每個 Worker Run 是一次 `claude -p`。Supervisor 啟動 Run 時依序做：持久化 launch intent → 取得並核對設定快照 → 組出 `RunSpec`（Prompt、工作目錄、Run 帳號 Profile 的設定目錄、快照目錄、Worker 模型；全部來自持久化的狀態，Runtime 重建後組出相同結果）→ `prepare` 寫入 Run 檔案 → 簽發 Agent 憑證 → spawn。

| 項目 | 內容 |
| --- | --- |
| 指令 | `claude -p --output-format stream-json --verbose --setting-sources "" --strict-mcp-config --mcp-config <Run 的 mcp.json> [<快照的 mcp.json>] --allowedTools mcp__baton --permission-prompt-tool mcp__baton__permission_prompt [--settings <快照的 settings.json>] [--append-system-prompt-file <快照的 CLAUDE.md>] [--model <模型>]` |
| 設定來源 | `--setting-sources ""`：使用者層、Workspace 內的 `.claude\settings.json` 與 `settings.local.json` 都不載入，Worker 無法藉由寫入這些檔案改變自己的權限（票 22 驗證 R052 的意見）；設定只來自 Run 的快照 |
| 權限 | 預設權限模式（不使用 `bypassPermissions`）；Baton 的 MCP 工具預先允許，其餘需要核准的操作經 `permission_prompt` 交給 Runtime 判定（spec D-08） |
| Run 檔案 | `<資料目錄>\runs\<run_id>\`：`prompt.md`（經標準輸入交付，不放進指令列）、`mcp.json`（只指向 shim，憑證以 `${BATON_AGENT_TOKEN}` 參照）、`stream.jsonl`（標準輸出）、`stderr.log` |
| 環境 | `CLAUDE_CONFIG_DIR` = Run 帳號 Profile 的目錄；移除 Runtime 繼承到的 `ANTHROPIC*`、`CLAUDE*` 變數（`CLAUDE_CODE_GIT_BASH_PATH` 除外）；Supervisor 再加上 `BATON_AGENT_TOKEN`、`BATON_RUNTIME_URL` |
| 憑證 | 啟動參數只有旗標、檔案路徑與模型名稱，**不含任何憑證值**（情境測試比對 Agent 憑證與 Profile 的登入資料） |
| 事件 | 從 `stream.jsonl` 追蹤（Runtime 重建後重新接上同一個檔案）：`system/init` → `Started`（原生 Session）、`assistant` 文字 → `Message`、`rate_limit_event` → `RateLimited`（見「額度觀測」）、`result` → `UsageReported`，失敗時再 `Error`，最後 `Exited`；有新輸出就是 `Heartbeat` |
| 原生 Session | 第一次回報時以 `BindNativeSession` 綁定到 Run（`run.native_session_id`，事件 `run_native_session_bound`）；之後相同不變，不同則只記錄 `run_native_session_conflict`，**不改綁定** |
| 失敗原因 | `result` 的 `is_error` 為 true（即使 `subtype` 是 `success`）時，Run 以 CLI 回報的文字作為 `end_reason` |
| resume | `resume_capability` 一律回報 `Unknown`（屬 Session 持久化與 resume 的票） |

ProcessControl（`windows_process.rs`）依 `SpawnSpec` 的 `env_remove`、`stdin_file`、`stdout_file`、`stderr_file` 處理環境與標準輸入輸出；輸出檔以 share read/write/delete 開啟，Adapter 可在程序執行中讀取。

**模型（spec D-30）**：平台預設不指定模型。Workspace 可依角色覆寫：`PUT /api/v1/workspaces/{id}/model-overrides`，本文 `{ "controller"?, "worker"?, "reviewer"? }`（`null`、省略或空白 = 不覆寫；名稱只允許英數字與 `. _ - : [ ] @ /`，否則 400 `model_name_invalid`；封存的 Workspace 409 `workspace_already_archived`），回 200 與 Workspace（`model_overrides`）。派工時把 Worker 角色的模型固定進派工 Outbox（`LaunchPayload.model`），所以**只影響之後派出的 Run**。UI 在 Workspace 的「授權設定」畫面下方。

**Adapter 契約測試**：`crates/baton-harness/tests/adapter_contract.rs`，同一份契約跑在 fake、replay（真實 CLI 錄下的 stream-json）與 live（真實 CLI，手動）上；執行方式與結果記錄見 [adapter-contract.md](adapter-contract.md)。

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

它會以暫存資料目錄與隨機埠啟動真正的 `baton-desktop.exe`，經由 WebView2 遠端除錯埠讀取畫面，確認：狀態列顯示「Runtime 運作中」、首次設定精靈以本機真實的 `claude`、`git` 檢查、帳號頁新增 Profile 並以真實的 `claude auth status` 核對、第 9 個 Profile 被拒絕、App 程序結束後 Runtime 仍在執行（關閉視窗只會隱藏到系統匣，所以測試直接結束 App 程序）、再次開啟時連回同一個 Runtime、未帶憑證的請求被拒絕。這個測試會開啟視窗，所以不納入 `pnpm test`。

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
- `harness.supervise()`：讓 Supervisor 處理一次（Harness 不啟動背景迴圈，何時發生由測試決定）；`process_control.hang_next_spawn_at(HangPoint::…)` 讓下一次 spawn 在「啟動前／啟動後回執前」永遠不回來，測試以 `timeout` 丟棄該次處理即等同 Runtime 在那一點崩潰；`fail_next_spawn_after_start(reason)` 讓下一次 spawn 真的啟動程序卻回傳失敗（回執遺失），`fail_finds(Some(reason))` 讓 `find` 一直失敗（無法查證），`run_on_next_spawn(action)` 在程序啟動之後、回執交還之前執行任意動作（例如注入寫入故障）；`harness.inject_storage_fault(Some(StorageFaultKind::DiskFull | Busy | Unwritable))` 讓目前 Runtime 的每個資料庫寫入都以該種故障失敗（`None` 恢復；`harness.store()` 取得 Store 本身）；`agent_adapter.emit(run_id, event)`、`script_next_run(events)` 推送 Adapter 事件。
- `harness.workspace_fs.hang_next_artifact_write_at(ArtifactWritePoint::MidWrite | BeforeRename | AfterRename)`：讓下一次成果寫入在「寫到一半／改名前／改名後（manifest 提交前）」永遠不回來；測試以 `timeout` 丟棄該請求、再 `harness.restart()`，即等同 Runtime 在那一點崩潰。`harness.artifacts_dir()` 回傳成果儲存根目錄（`<資料目錄>\artifacts`），用來確認磁碟上留下哪些檔案。
- `harness.workspace_fs` 的 Git 腳本（票 20）：`init_repo(dir, branch)` 把目錄當成簽出該分支的 repo（分支在 commit `base`）、`detach_head`、`fail_next_worktree(reason)`、`conflict_next_merge(detail)`、`hold_next_merge()`（下一次合併停住直到放行；不放行並以 `timeout` 丟棄即等同崩潰）、`move_branch`、`branch_tip`、`worktree_branch`、`merges()`（每次整合合併的紀錄）。`command_runner.script_exit_in(program, dir, code, output)` 讓同一個命令在某個工作目錄（例如整合目錄）有不同結果。
- `harness.restart()`：模擬重啟。舊的 Runtime 被丟棄（原位址不再回應），資料目錄與全部 fake 都保留。
- 完全退出：`POST /api/v1/lifecycle/exit` 之後以 `harness.wait_for_exit()` 等停止流程結束（Harness 的 Runtime 在退出後不會自行結束服務，由測試決定何時 `restart`）；`Harness::builder().exit_timing(..)` 縮短 30 + 10 秒的上限；`process_control.script_stop(identity, FakeStop::Result(..)／Hang／Fail(..))` 指定某個程序停不停得下來，`stop_graces()` 讀取每次停止收到的寬限期；`harness.lifecycle_file_path()` 是生命週期檔的位置。
- `Harness::builder().without_full_text_search()`：以 `RuntimeOptions.full_text_search = false` 啟動，視同 FTS5 不可用，知識搜尋退回 LIKE（`search_mode = like_fallback`，日誌記錄降級）；之後 `harness.set_full_text_search(true)` 再 `harness.restart()`，即模擬 FTS5 恢復可用、啟動時重建索引。
- `harness.clock`、`process_control`、`agent_adapter`、`quota_observer`、`workspace_fs`、`platform_info`、`secret_protector`、`claude_env`：具體的 fake，可腳本化回應並讀取呼叫紀錄。`claude_env` 可設定 Claude Code／Git 是否存在與版本（`set_claude`、`set_git`、`fail_tool_probes`）、在某個設定目錄完成原生登入（`log_in`，會像真實的 Claude Code 一樣把登入資料寫進該目錄）、模擬環境變數覆蓋（`set_env_override`）、讓開啟登入終端機時順便登入（`script_terminal_login`），並記錄開啟過的終端機（`login_terminals`）。
- 示範測試：`crates/baton-harness/tests/health_demo.rs`；本機 API 防護情境：`crates/baton-harness/tests/api_auth.rs`；帳號 Profile 與首次啟動精靈情境：`crates/baton-harness/tests/profiles_and_first_run.rs`；Workspace 與控制資料庫情境：`crates/baton-harness/tests/workspaces.rs`；Task 與契約情境：`crates/baton-harness/tests/tasks.rs`；Supervisor、Run、程序身分與取消情境：`crates/baton-harness/tests/runs.rs`；裁定中心與權限提示情境：`crates/baton-harness/tests/decisions.rs`；單次批准（E1）與修改授權情境：`crates/baton-harness/tests/one_off_approval.rs`；Agent API、範圍憑證與 MCP shim 情境：`crates/baton-harness/tests/agent_api.rs`（以 `tokio::io::duplex` 當 stdio，在程序內跑真正的 shim 對真實 API 回報）；Artifact 儲存、manifest、中斷恢復與下載驗證情境：`crates/baton-harness/tests/artifacts.rs`；多帳號排程情境：`crates/baton-harness/tests/scheduling.rs`；額度觀測情境：`crates/baton-harness/tests/quota.rs`；知識、教訓與全文檢索（含降級）情境：`crates/baton-harness/tests/knowledge.rs`；Guard 與重犯事件情境：`crates/baton-harness/tests/guards.rs`；Outbox 核對、外部操作與寫入故障情境：`crates/baton-harness/tests/outbox_and_external_ops.rs`；桌面生命週期與完全退出情境：`crates/baton-harness/tests/lifecycle.rs`，關窗與重開視窗連回同一個 Runtime：`crates/baton-client/tests/lifecycle_client.rs`；Worktree、輸出目錄與整合佇列情境：`crates/baton-harness/tests/worktrees.rs`。

後續的票撰寫情境測試時，一律透過這個 harness 操作真實 API；不要直接呼叫內部函式或斷言資料表欄位。

### CI

`.github/workflows/ci.yml` 在 `windows-latest` 上執行 `pnpm test` 與 `pnpm build`。

## 產品名稱

產品名稱集中在 `crates/baton-domain/src/product.rs`（`PRODUCT_NAME`、`DATA_DIR_NAME`）與安裝設定 `src-tauri/tauri.conf.json`（`productName`、視窗 `title`）。`crates/baton-domain/tests/product_name.rs` 會檢查兩處一致；Web UI 畫面上的名稱取自 Runtime 回報，不寫死。改名時只需修改這幾處（以及既有使用者的資料目錄）。

## 平台辨識與支援期限表

- PlatformInfo Port 只回報事實（OS build、功能版本、版本別、原生架構、程序架構）；支援狀態由 `baton-domain::platform::assess` 依事實與 Clock 判定，所以到期判斷可用 fake Clock 測試。
- 正式實作在 `crates/baton-runtime/src/windows_platform.rs`：Windows 11 以 build ≥ 22000 判定（登錄的 `ProductName` 在 Windows 11 上仍可能寫 Windows 10），原生架構取自 `IsWow64Process2`。
- 支援期限表（`SUPPORT_TABLE`）的來源與查閱日期寫在同一檔案；新增驗證通過的功能版本、或查到官方日期時，一併更新 `SUPPORT_TABLE_CHECKED_ON`。
