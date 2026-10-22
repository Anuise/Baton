# 開發者指南

Baton（暫名，見 spec D-01）是 Windows 桌面應用程式：Rust 寫的常駐 Runtime、React + TypeScript + Vite 的 Web UI、Tauri 桌面殼。舊版 skill（`skills/`、README）在重建期間不動，已用 git tag `archive/legacy-skill` 封存。

## 目錄

| 路徑 | 內容 |
| --- | --- |
| `crates/baton-domain` | Domain 核心（純邏輯、無 I/O）：產品名稱常數、時間戳、Runtime 設定解析、平台支援狀態判定與支援期限表（`platform.rs`）、Workspace 狀態機（`workspace.rs`：命令 → 新狀態 + 事件）、Workspace 授權範本／確認 revision／Run 准入檢查（`authorization.rs`）、Goal／Task 狀態機與六項最小派工契約、依賴未決答案的准入缺項（`task.rs`）、Run 狀態機／execution lease／派工 Outbox（`run.rs`）、多帳號排程與容量（`scheduler.rs`：選帳號、並行上限、交接槽、等待原因）、額度觀測與判斷（`quota.rs`：已觀測／推估、較保守者、觀測不完整）、帳號額度門檻（遲滯、draining）與主動交接的 phase 狀態機、冷卻與安全邊界、突然限流與等待資源的重新評估時間（`handoff.rs`）、account_profile 狀態機與可派工判定（`profile.rs`）、首次啟動精靈的工具檢查與背景行為告知（`setup.rs`）、資料目錄位置判定（`storage_location.rs`）、Agent 工具介面（`agent.rs`：環境變數名稱、MCP 設定、Worker 回報內容與長度上限）、Artifact 的登記驗證、版本 manifest、未登記檔案辨識與預覽判定（`artifact.rs`）、確定性檢查與成果接受交易（`verification.rs`）、共同設定的匯入、秘密遮蔽、差異、版本與 Run 快照渲染（`shared_settings.rs`，秘密辨識在 `shared_settings/secrets.rs`）、裁定請求與裁定的狀態機（`decision.rs`：版本校驗、六種處理方式、合併與連結、Controller 動作判定、Controller 提出的請求）、決策權限表（`authority.rs`，spec D-15）、權限提示的歸類與授權判定（`permission.rs`）、單次批准（E1）的綁定與核對（`approval.rs`）、備份的觸發／保留／還原演練核對（`backup.rs`）、知識條目與教訓的可信狀態轉換、版本、取代關係、可見範圍與搜尋關鍵字（`knowledge.rs`）、Guard 規則、啟用停用、Run 准入時的違反判定與重犯事件（`guard.rs`）、Context Builder 的 Prompt 組裝、相關性排序、token 預算與 context receipt（`context.rs`）、桌面生命週期與完全退出（`lifecycle.rs`：停止意圖、停止報告與停止異常、各 Workspace 的自動續跑與暫停、生命週期檔）、Task 工作目錄（worktree／輸出目錄）的位置與登記（`workdir.rs`）、單一整合佇列的狀態機（`integration.rs`）、外部操作的冪等鍵、要求雜湊、回執與結果未知（`external_operation.rs`）、控制資料庫寫入故障的種類與處理方式（`storage_health.rs`）、Agent Session 的沿用／替換決定、條件綁定與對話紀錄的去重和遮蔽（`session.rs`）、Controller 長期對話的輸入、喚起與輪次（`conversation.rs`）、修正與重試預算的計數／退避／重設（`budget.rs`，spec D-16）、Runtime 意外崩潰後的自動恢復（`recovery.rs`：恢復事件、每輪三次嘗試與退避、穩定觀察、用盡與人工重試的輪次、派工阻擋、監護嘗試的匯入）、監護程序的規則（`guardian.rs`：監護日誌紀錄、恢復狀態鏡像、抑制判定、重啟規劃、監護狀態）、跨 Windows 登入的啟動與續跑（`login.rs`：登入時自動啟動的偏好、各 Workspace 的跨登入自動續跑、登入邊界判定與暫停）、事件時間線的分類與依 Task 過濾（`timeline.rs`）、由時間線事件計算的觀測指標（`metrics.rs`） |
| `crates/baton-ports` | 所有外部副作用的 Port 介面：Clock、ProcessControl（含 `find`）、AgentAdapter（含 `spawn_spec`）、QuotaObserver、WorkspaceFs（含成果的 `write_artifact_atomic`、`read_artifact`、`list_artifact_files`，以及 `write_readonly_files`：把一組檔案寫成唯讀目錄）、PlatformInfo、SecretProtector、ClaudeEnvironment、CommandRunner（驗證命令）、UserSettingsSource（唯讀讀取使用者平常使用的 Claude Code 設定，只有白名單內的三個檔案）、GuardianProbe（監護程序是否還在，只用於顯示降級）、LoginSession（目前 Windows 登入工作階段的識別）、StartupRegistration（登入時自動啟動的 HKCU Run 登記）；`write_file_atomic`（暫存 → fsync → 原子改名）是成果寫入的真實實作 |
| `crates/baton-store` | SQLite 存取：單一寫入佇列、PRAGMA 讀回驗證、`schema_migrations`、Workspace、account_profile、授權 revision、Goal／Task／契約版本／相依與事件表、Run／lease／Outbox 表、派工佇列與容量設定（`scheduling.rs`）、成果 manifest（`artifact`、`artifact_version`）、驗證紀錄（`task_verification`）、共同設定版本表 `shared_settings_version` 與 Run 設定快照綁定表 `run_settings_snapshot`（都是追加式）、額度觀測／本機用量／方案上限（`quota.rs`）、額度門檻與交接紀錄（`handoffs.rs`：`quota_gate`、`handoff`）、裁定請求／裁定／單次批准／連結與 Workspace 決策權限表（`decisions.rs`）、備份／還原／還原演練（`backup.rs`）、知識條目與版本（`knowledge.rs`：`knowledge_entry`、`knowledge_entry_version`，以及每次開啟時重建的全文檢索索引 `knowledge_fts`）、Guard 與重犯事件（`guards.rs`：`guard`、`lesson_recurrence`）、Context Builder 的輸入取得與 context receipt（`context.rs`：`context_receipt`）、停止意圖與 Workspace 生命週期設定、生命週期檔的原子讀寫（`lifecycle.rs`）、外部操作（`external_operations.rs`：`external_operation`，Run 結束與回答裁定的同一個交易中結算）、寫入故障的歸類與試寫（`health.rs`：`storage_probe`）、Agent Session 與對話紀錄段落（`sessions.rs`：`agent_session`、`session_events` 檢視、`transcript_segment`）、Controller 對話（`conversations.rs`：`conversation`、`conversation_turn`、`controller_run`）、修正與重試預算（`budgets.rs`：`task_budget`）、崩潰恢復事件與嘗試（`recovery.rs`：`recovery_event`、`recovery_attempt`，以及 `runtime_boot.ended_cleanly_at` 正常結束標記；監護日誌的匯入紀錄 `guardian_journal_import` 與給監護看的恢復狀態鏡像）、跨登入（`lifecycle.rs`：`runtime_boot.login_session`、`workspace_lifecycle.auto_resume_across_login`／`paused_by`、`login_startup`）、時間線的讀取與 Task 歸屬（`timeline.rs`，唯讀，沒有自己的資料表） |
| `crates/baton-api` | Axum 路由（`/api/v1/health`、`/api/v1/platform`、`/api/v1/workspaces`（含授權、准入檢查、Goal、Task、Run、Run 的 context receipt、Run 設定快照、成果瀏覽與成果接受）、`/api/v1/scheduling`、`/api/v1/lifecycle`（含 `/api/v1/lifecycle/login-startup` 與 `/api/v1/workspaces/{id}/login-resume`）、`/api/v1/recovery`、`/api/v1/decisions/pending`、`/api/v1/decisions/batch`（與 `/api/v1/workspaces/{id}/decisions`、`/api/v1/workspaces/{id}/decision-authority`）、`/api/v1/profiles`、`/api/v1/setup`、`/api/v1/shared-settings`（共同設定的預覽、採用與版本）、`/api/v1/backups`（備份與還原演練狀態）、`/api/v1/knowledge`、`/api/v1/guards`（與 `/api/v1/knowledge/{id}/recurrences`）、`/api/v1/workspaces/{id}/timeline` 與 `/api/v1/workspaces/{id}/metrics`、`/api/v1/events`）與入口防護 `guard`（憑證、角色、Host、Origin、CORS）；Agent API（`/agent/v1/tools/…`，`agent.rs`，含 `controller_action`、`register_artifact`、`confirm_handoff`；`knowledge.rs` 的 `record_lesson_candidate`、`search_knowledge`；`controller.rs` 的 Controller 專屬工具） |
| `crates/baton-credential` | 桌面憑證檔（`auth/desktop-credential.bin`）的讀寫與 DPAPI 保護 |
| `crates/baton-runtime` | Runtime 組裝（`Runtime::start`，`RuntimeOptions` 含 `full_text_search`；含啟動時核對停止意圖、判定上次是否意外終止，與完全退出的執行者）、Supervisor（`supervisor.rs`，含完全退出的停止流程與重新開啟、崩潰恢復的嘗試、主動交接）、Claude Code Adapter（`claude_adapter.rs`）、備份排程（`backup.rs`）、確定性檢查的執行（`verifier.rs`）、Windows Job Object 的 ProcessControl（`windows_process.rs`）、單實例鎖、`baton-runtime.exe`（`baton-runtime restore …` 還原備份） |
| `crates/baton-guardian` | 監護程序（`baton-guardian.exe`，票 32）：監護狀態機 `Guardian`（以 `RuntimeHost` 抽象 Runtime）、監護日誌／恢復狀態鏡像／生命週期檔／程序身分的檔案讀寫（`files.rs`）、以程序識別碼與記錄時間確認程序還在（`process.rs`）、正式版的 `ProcessHost`、登入啟動入口（`launcher.rs`，`baton-guardian.exe --login`，票 33）；不依賴 `baton-store`（不開控制資料庫） |
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

產物在 `target/release/`：`baton-runtime.exe`（Runtime）、`baton-guardian.exe`（監護程序）與 `baton-desktop.exe`（桌面 App，內嵌 `web/dist`）。三個檔案必須放在同一個資料夾，App 啟動時會到自己旁邊找 Runtime 與監護。

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

revision 存在 append-only 的 `authorization_revision` 表（觸發器禁止更新與刪除），並在同一交易追加 `authorization_confirmed` 事件（聚合版本 = revision 編號）。准入判定 `authorization::admit` 由 `admission-check` 端點、派工交易（`RunCommand::Dispatch`）與啟動准入（`RunCommand::BeginLaunch`）共用，不另寫准入邏輯。`admission-check` 也走寫入佇列，結果與授權更新依同一個持久化順序排列。

**授權撤銷與啟動准入順序（R1-a，票 24）**：Run 的「准入」是 Supervisor 持久化 launch intent 的那一個交易（`BeginLaunch`），不是派工。授權更新與准入都經過單一寫入佇列，所以兩者的先後就是持久化順序，事件表的 `authorization_confirmed` 與 `run_launch_started`／`run_admission_denied` 的 `seq` 可以查驗：

- 准入時以**當下生效**的授權重新檢查 Task 契約需要的能力。通過就把 Run 的 `authorization_revision` 改寫為當下的 revision，並記下 `admitted_at` 與 `settings_version`（`run` 表的兩欄，migration 23 `run_admission`；設定版本取已綁定快照的版本，沒有綁定時取最新採用的共同設定版本）；之後不再改變。
- 被拒（排隊中或已有派工 Outbox、但撤權發生在准入之前）：不寫 launch intent、不簽發憑證、不啟動程序；Run 以 `interrupted`（`end_reason` 以 `admission_denied` 開頭，事件 `run_admission_denied`）結束、釋放寫入所有權，Task 回到 `ready` 並在同一個交易中重新排隊，排程以「授權」類別顯示等待原因。
- 已准入的 Run 沿用准入時的 revision：撤權不會停止它，權限提示也依它的 revision 判定（新增的範圍同樣不會套用到它）。恢復 Run、同一 Task 的新 Run 與其他工作一律在准入時取得新版，舊 Run 不能成為權限捷徑。
- `GET …/authorization` 的 `legacy_runs` 列出「已准入、尚未結束、沿用較舊 revision，而且舊 revision 有目前已撤銷或縮小的能力」的 Run（`revoked_capabilities`、`cancel_requested`），授權畫面顯示「後續執行已停用，仍有 N 個 Run 沿用舊授權」與「暫停」（取消這個 Run，Task 回到可派工）／「取消」（取消 Task）入口；工作面板 `GET …/tasks` 帶目前的 `authorization_revision`，Run 卡片標示「沿用舊授權（revision X，目前 Y）」。

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

**Run 狀態**：`queued → starting → running → succeeded／failed／interrupted／cancelled`（`RunStatus::allowed_next`）。只有 `running` 之後才有程序身分；`starting` 代表 launch intent 已持久化（也就是已准入）但回執還沒寫回。`queued` 的 Run 啟動准入被拒時直接進入 `interrupted`（見「授權撤銷與啟動准入順序」）。

**派工交易**（Domain `RunCommand::Dispatch`，帶 Scheduler 選定的 `profile_id`）：在同一個交易中重新做完整准入檢查（`run::check_dispatch`：`task::admission_report`、`authorization::admit`、適用的 Guard 都沒有違反（見〈Guard 與執行前檢查〉）、相依都已完成、沒有任何尚未釋放的 lease），然後建立 Run（`queued`，記下使用的帳號）、execution lease（世代 = 該 Task 的 Run 數 + 1）、派工 Outbox 項目，Task 進入 `running`。派工**不啟動程序**。誰來選帳號、容量不夠時怎麼辦，見下一節。

**寫入所有權**：同一個 Task 任何時刻最多一個尚未釋放的 `execution_lease`（Domain 判定 + 資料庫唯一索引 `execution_lease_one_held_per_task`；性質測試 `at_most_one_effective_writer_per_task_under_any_command_sequence`）。Run 結束或取消**確認程序停止之後**才釋放；停不掉（`StopResult::StillAlive`）就一直持有，新的 Run 派不出去。

**Supervisor**（`Supervisor::tick`，每次依序做八件事，全部冪等、只信任資料庫）：

1. 啟動派工 Outbox 的 Run（Outbox Dispatcher，至少一次交付、以 `command_id` 去重）：先在交易中持久化 launch intent 與 `command_id`（`starting`），才在交易外 `spawn`，再把回執（PID + 建立時間 + 指令雜湊）寫回（`running`）。寫入 launch intent 的交易再檢查一次停止意圖（`launch_halted`）與**准入**（目前生效的授權仍涵蓋契約需要的能力、Workspace 沒有封存）；准入不成立時 Run 以 `interrupted`（`admission_denied：…`）結束、Outbox 視為處理完畢，不簽發憑證也不啟動程序（見「授權撤銷與啟動准入順序」）。`spawn` 回報失敗時先以 `command_id` 呼叫 `ProcessControl::find` 查證：找到（例如回執在傳回途中遺失）就補寫回執；確定沒有才判定 `launch_failed`；無法查證就留在 `starting` 等下一次核對。Runtime 在任何一點當掉，重建後以 `command_id` 找回原程序並補寫回執，**不重複啟動**；找不到就把 Run 標為 `interrupted`（`launch_outcome_unknown`），不自動重送，由使用者重新派工；`find` 本身失敗（無法證明程序在不在）時 Run 留在 `starting`、繼續持有寫入所有權，**不中斷也不重派**，直到查得到為止。控制資料庫無法寫入時（見〈外部操作與寫入故障〉）這一步與第 6、7 步都暫停。
2. 收 Adapter 事件：`Heartbeat` 續約 lease（間隔 15 秒、有效 60 秒，spec D-31）；`RateLimited`、`UsageReported` 記為該 Run 帳號的額度觀測（見「額度觀測」）；`Exited` 只結束 Run（成功 → Task `verifying`，失敗 → Task 回到 `ready`），**不會**把 Task 標為 `completed`；正在交接的 Run 結束時不論結束碼都視同舊程序已停止，直接轉移給 successor；帳號已達上限（突然限流）之後以失敗結束、還沒有交接的 Run，在這一步建立交接並轉移（或等待資源）（見〈檢查點與主動交接〉）。
3. 取消：要求取消的 Run 先記下意圖（`cancel_requested_at`），Supervisor 停止整個程序樹，確認之後才完成取消、釋放 lease、Task 回到 `ready`；還沒啟動的 Run 立即取消。取消 Task 會在同一個交易中要求取消它進行中的 Run。
4. 核對 lease：過期只觸發核對（`inspect`）：確認 `Dead` 才中斷 Run，`Alive` 續約，`Unknown` 維持原狀。
5. 額度：輪詢到期（每個帳號每 5 分鐘）的外部額度觀測器並重新推估（見「額度觀測」）。
6. 交接：推進帳號的額度門檻，重新評估到期的等待資源交接，為 draining 帳號上執行中的 Run 建立交接、在安全邊界要求舊程序停止、確認停止後轉移給 successor（見〈檢查點與主動交接〉）。
7. 排程：前面幾步可能釋放了名額，於是讓派工佇列依序再試一次（見下一節）；交接與排程派出的 Run 在同一次 `tick` 就啟動。
8. 驗證：對 `verifying` 且契約宣告了確定性檢查的 Task 執行檢查並提交結果（見「成果接受與最小完成驗證」）；背景迴圈把這一步放在另一個工作執行。

正式執行時由背景迴圈每秒（或派工／取消時被喚醒）呼叫 `tick`；`RuntimeOptions { background_supervisor: false }`（harness 使用）不啟動背景迴圈，由測試呼叫 `harness.supervise()`。`RuntimeOptions::stop_grace` 預設 30 秒（spec D-11）。

**真實 ProcessControl**（`WindowsProcessControl`，只在 Windows 編譯）：每個 Run 一個具名 Job Object `Local\Baton-Run-{command_id}`（不設 KILL_ON_JOB_CLOSE，Runtime 崩潰不會殺掉程序）；`stop` 先在寬限期內等程序自行結束，逾時 `TerminateJobObject` 終止整棵樹。整合測試：`crates/baton-runtime/tests/process_tree.rs`（直接測 Port）、`crates/baton-runtime/tests/supervisor_process_tree.rs`（真實 Runtime + HTTP 取消）。正式執行檔的 AgentAdapter 是 Claude Code Adapter（見「Claude Code Adapter」）。

桌面憑證專用的端點（Agent 憑證得到 403），在 `/api/v1/workspaces/{id}/` 之下：

| 端點 | 用途 |
| --- | --- |
| `POST tasks/{task_id}/dispatch` | 派工要求（Task 必須是 ready，本文可帶 `{"priority": n}`）；有帳號與名額 → 201 回 `queued` 的 Run（含 `profile_id`）；否則 202 回 `{queued: true, reasons}`，Task 留在派工佇列。409 代碼：`task_not_ready`、`task_admission_denied`（契約失效，帶 `missing`）、`execution_conflict` |
| `GET runs`、`GET runs/{run_id}` | Run（含 lease、程序身分、`cancel_requested_at`、`end_reason`）；工作面板 `GET tasks` 也帶 `runs` |
| `POST runs/{run_id}/cancel` | 要求取消；已結束或重複取消是冪等的（200） |

事件（`aggregate_type = 'run'`）：`run_created`、`run_launch_started`、`run_started`、`run_lease_verified`、`run_succeeded`、`run_failed`、`run_interrupted`、`run_cancel_requested`、`run_cancelled`、`run_handed_off`（交接：`interrupted`、`end_reason` = `handed_off`，payload 帶 `handoff_id`）（heartbeat 只更新 lease，不追加事件）。

### Agent API、範圍憑證與 MCP shim

Worker 不直接碰資料庫或管理 API：它的程序啟動時帶著一組**只屬於這個 Run** 的憑證，由 MCP shim 把工具呼叫轉送到 Runtime 的 Agent API。純邏輯在 `baton_domain::agent`，端點在 `baton-api` 的 `agent.rs`，持久化在 `baton-store` 的 `agent.rs`。

**憑證怎麼交給程序**：Supervisor 在 `BeginLaunch`（launch intent 已持久化）之後、`spawn` 之前，呼叫 `CredentialIssuer::issue_agent` 為該 Run 簽發 Agent 憑證，綁定 Workspace、Run 與世代。它沒有時間上限，有效與否由「這個 Run 仍是寫入所有者」決定（所以 Runtime 重啟後，Worker 程序手上的憑證仍可用）。簽發失敗就不啟動程序。憑證只經 `SpawnSpec.env` 的環境變數 `BATON_AGENT_TOKEN` 與 `BATON_RUNTIME_URL` 交給程序，**不**進 Prompt、指令列、MCP 設定檔、日誌或資料庫明文（資料庫只存雜湊）。`SpawnSpec` 的 `Debug` 只印環境變數名稱，環境變數不納入 `command_hash`；真實 `WindowsProcessControl` 以 Unicode 環境區塊傳入（同名時以 `spec.env` 為準）。

**MCP 設定**：`baton_domain::agent::mcp_config_json(shim 路徑)` 產生的設定只含 shim 指令與 `${BATON_AGENT_TOKEN}`、`${BATON_RUNTIME_URL}` 的參照，由 Claude Code 展開；真實 AgentAdapter 為每個 Run 把它寫成 `runs\<run_id>\mcp.json`。

**兩組 API 分流**：`POST /agent/v1/tools/{工具}` 只接受 Agent 角色，管理 API（`/api/v1/…`）只接受桌面角色；`guard` 依路徑前綴 `/agent/v1/` 決定所需角色，角色不符回 403 `role_not_allowed`，Agent 憑證沒有範圍回 401 `credential_invalid`。身分與資源歸屬一律取自憑證；請求可以選擇帶 `workspace_id`／`run_id`，與憑證不一致就回 403 `agent_scope_mismatch`，不會悄悄改用請求裡的值。Agent 角色另有 Controller 憑證（票 17，綁定 Controller 的一輪）：它只能使用 Controller 專屬的工具，呼叫下表的 Worker 工具回 403 `worker_only`；Worker 憑證呼叫 Controller 工具回 403 `controller_only`，見〈Controller 拆工與派工工具（票 17）〉。

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
| `confirm_handoff` | `{ "completed", "incomplete", "unknown_side_effects" }` | 交接的 successor 確認接手：已完成、未完成與結果不明的副作用。細節見〈檢查點與主動交接〉 |

三個回報工具成功時回 `{ "accepted": true, "kind", "stop_requested", "handoff_requested" }`（`stop_requested`：完全退出的停止意圖生效，或這個 Run 正在交接，Worker 應儘快保存 checkpoint 並結束；`handoff_requested`：停止的原因是交接），同時續約 lease，並追加 Run 事件 `run_progress_reported`、`run_checkpoint_saved`、`run_blocked_reported`。錯誤：內容無效 400 `report_invalid`（空白或超過上限；上限 `SUMMARY_MAX_CHARS`、`REASON_MAX_CHARS`、`DETAILS_MAX_BYTES` 見 `baton_domain::agent`）、JSON 不合法 400 `bad_request`。

**過期世代**：Run 已結束或已被接替之後的回報回 409 `run_report_stale`，**不改變任何目前狀態**（Task、lease、事件都不動），但內容仍以 `accepted = 0` 留在追加式資料表 `agent_report`（觸發器禁止更新與刪除）供稽核；被接受的回報同樣留在該表（`accepted = 1`）。

**面板**：`RunView`（`GET runs`、`GET tasks` 的 `runs`）多了 `latest_progress`、`latest_checkpoint`、`blocked`，各取該 Run 最新一筆**被接受**的回報；工作面板（`TaskBoard.tsx`）的卡片顯示它們。

**MCP shim**（`crates/baton-mcp-shim`）：手寫的 stdio JSON-RPC 2.0（逐行；stdout 只輸出協定訊息，日誌走 stderr），只依賴 `baton-domain`，不依賴 `baton-api` 或 Runtime。`serve(input, output, ShimConfig)` 可在程序內以任意讀寫串流測試。啟動時從環境變數讀 `BATON_AGENT_TOKEN`、`BATON_RUNTIME_URL`，缺少或空白時只印變數名稱並以 exit code 2 結束；輸出（含錯誤與 `Debug`）不含憑證。

### 裁定中心與權限提示轉接

Worker 需要使用者決定的事、超出授權範圍的操作，以及 Controller 想自行處理但依決策權限表必須送裁定的事項，都變成**裁定請求**，集中在裁定中心等使用者處理。純邏輯在 `baton_domain::decision`（狀態機）、`baton_domain::permission`（權限提示判定）與 `baton_domain::authority`（決策權限表），持久化在 `baton-store` 的 `decisions.rs`，端點在 `baton-api` 的 `decisions.rs`（桌面）與 `agent.rs`（Agent 工具），UI 在 `web/src/DecisionCenter.tsx`。

**四條不變條件**（每一條都有情境測試，`crates/baton-harness/tests/decisions.rs`；完整裁定中心的情境在 `crates/baton-harness/tests/decision_center_full.rs`）：

1. **Agent 不能裁定**：回答只有桌面憑證的端點（`/api/v1/…`，Agent 憑證在 `guard` 就得到 403 `role_not_allowed`）；Agent API 沒有任何回答或批准的工具；`decision.decided_by` 在資料庫層 CHECK 為 `desktop`。
2. **只阻擋受影響的 Task**：請求的 `blocking` 明列受影響的 Task，只有它們進入 `waiting_decision`；回答後只有仍在 `waiting_decision` 而且**沒有其他待處理請求也阻擋它**的 Task 回到 `ready`，並在同一個交易重新排進派工佇列（`dispatch_request`，優先序 0），由 Scheduler 自動派出新的 Run；舊 Run 還持有 execution lease 時留在佇列等待（原因 `execution_conflict`），舊 Run 結束後的下一次排程派出。其他 Task 從頭到尾不受影響。
3. **逾時與延後都不是同意**：`deadline_minutes` 只讓 `overdue` 變成 `true`，延後只改變排序與 `deferred`；沒有任何程式路徑會因時間流逝回答請求、放行權限提示或恢復 Task。
4. **一次交易**：回答時，裁定、請求狀態、受影響 Task 的狀態與事件、重新排隊、喚醒 Outbox 在同一個交易提交。

**Agent 工具**（`POST /agent/v1/tools/{工具}`，範圍取自憑證，只有仍持有 execution lease 的 Run 能呼叫；否則 409 `run_report_stale`）：

| 工具 | 請求 | 行為 |
| --- | --- | --- |
| `request_decision` | `question`、`reason`、`options`（2–8 個 `{id, label, tradeoff?}`）、`recommendation?`、`evidence?`、`urgency?`（`low`／`normal`／`high`）、`deadline_minutes?` | 建立請求；Task（`running` 時）進入 `waiting_decision`，Run 不結束。回 `{decision_request_id, status: "waiting_decision", existing, message}`；同一個 Run 對同一個問題重複提出時回到既有請求。內容無效 400 `decision_invalid`，不寫入任何東西 |
| `permission_prompt` | `tool_name`、`input`（Claude Code `--permission-prompt-tool` 協定） | 授權範圍內：回 `{behavior: "allow", updatedInput, status: "auto_allowed"}` 並追加稽核事件 `permission_auto_allowed`。範圍外：建立裁定請求（選項固定為 `allow_once`、`deny`）並回 `{behavior: "deny", message: "等待裁定…", status: "waiting_decision", decision_request_id}`。使用者已回答時，同一個 Task 的同一個操作消耗一次答案：`approved`（允許）或 `denied`（拒絕）。先前的單次批准不能用時，等待裁定的回應另帶 `approval_check: {decision_request_id, result}`（`params_changed`、`target_changed`、`artifact_version_changed`…或 `used_result_unknown`） |

| `controller_action` | `matter`（決策權限表的事項）、`task_id?`（同 Workspace，省略時是自己的 Task）、`summary`、`reason`、`evidence?`、`urgency?` | **不執行任何動作**，只依 Workspace 目前生效的權限表判定：`allowed`（可自主處理）、`requires_decision`（動作被拒絕，已建立 `kind: "authority"` 的裁定請求，選項 `approve`／`reject`，阻擋目標 Task；同一 Task 同一事項仍待處理時回既有請求）、`approved`（使用者以接受或修改條件核准，消耗一次，`conditions` 是修改後的條件）、`denied`（消耗一次）、`human_only`（只能人工發起，不建立請求）。細節見 AI-227 |

**決策權限表**（`baton_domain::authority`，spec D-15，AI-227、AI-228）：內建 17 個事項，7 個可自主處理、9 個必須送裁定、1 個只能人工發起（三次恢復失敗後的重試）。Workspace 只能把可自主處理的事項收窄為必須送裁定；放寬或調整只能人工發起的事項一律 400 `authority_widening_refused`。設定存在 `decision_authority_policy`（只記被收窄的事項，`revision` 從 0 起，`PUT` 取代式並以 `expected_revision` 校驗）。Controller 專屬的變更工具（票 16、17）必須由 Runtime 依實際變更推導事項再判定，不能信任 Agent 自報的 `matter`。

**權限提示的判定**（`permission::evaluate`，保守的詞法判定，細節與取捨見 AI-138）：把「工具名稱＋輸入」歸類為能力（讀取／編輯檔案、讀取公開網頁、建置測試、安裝專案依賴、全域安裝、遠端寫入），再對照 **Run 准入時固定的授權 revision**（R1-a，票 24；准入時還沒有任何授權的 Run 一律範圍外）；檔案路徑必須落在授權目錄內，寫入 `.git`、本機或內網網址、含 shell 組合語法的指令、未知工具、尚未確認授權一律範圍外。Bash 指令以 bash 規則斷詞（去掉引號；引號不成對或反斜線緊接引號一律無法歸類），每個子命令只接受白名單內的旗標，任何其他旗標（例如 `git commit --amend`、`git diff --output=…`、`npm install --prefix …`、`cargo test --manifest-path …`）都無法歸類；`-g`、`--global=…`、`--location=global` 等全域安裝的各種寫法歸為「全域安裝」；指令參數中像路徑的字詞必須在 Workspace 內。路徑元件比對前依 Win32 規則去掉結尾的 `.` 與空白（`.git.`、`.git ` 視同 `.git`），含 `:`（替代資料流）的元件、只有點的元件、`\\?\` 裝置路徑一律範圍外，`GIT~1` 視同 `.git`。單次批准的規則見 AI-139。

**單次批准（E1，票 23）**（`baton_domain::approval`，細節與取捨見 AI-180～AI-182）：權限提示的請求建立時就算好它綁定的**單一邏輯操作**——操作識別（工具名稱，Bash 另加程式與子命令，例如 `Bash:git push`）、目標資源（檔案路徑、網址，或指令中非旗標的參數）、重要參數（其餘輸入與旗標）、要求雜湊（工具名稱與輸入的 SHA-256）、成果版本（提出請求的 Task 已登記成果的最新版本，`名稱@版本`）；Claude Code 附上的 `description` 說明欄位不列入。「允許本次操作」只綁 Task，不綁 Run 或帳號，所以恢復後換帳號、換 Run 的 Worker 可以沿用；Worker 再次提示時**逐項核對**，任何一項不同就不消耗批准、改建新的請求，並在回應與新請求的 `permission.rejected_approval` 說明哪一項變了。批准用過一次就失效；之後再提出同一個操作時，Runtime 看不到上一次的結果，新的請求帶 `permission.prior_use`、建議拒絕、選項改為「已查證，允許再執行一次」，要求使用者先查證。單次批准從不改變 Workspace 授權；要永久放行得走「修改 Workspace 授權」（摘要的 `changes` 與 `acknowledged_expansion`），修改授權不會回答裁定請求。

**桌面端點**（桌面憑證專用）：

| 端點 | 用途 |
| --- | --- |
| `GET /api/v1/workspaces/{id}/decisions` | 先把目標版本已變更的待處理請求標為 `superseded`，再回 `{pending_count, overview, decisions}`；`overview` 是總覽數字（待處理、可處理、延後、逾期、高緊急、已回答、已過期、已合併）；可處理的在前（緊急程度高者優先），延後的其次，已結束的在後 |
| `GET …/decisions/{request_id}` | 單筆詳情：問題、原因、選項與取捨、建議、證據、阻擋範圍（受影響的 Task 與狀態）、目標版本 `target`、內容版本 `revision`、`allowed_actions`、`links`／`merged_from`／`merged_into`、`deferred_until`、`status_reason`、Controller 動作細節 `authority`、權限提示細節（工具、輸入、能力、單次批准的 `binding`、`prior_use`、`rejected_approval`、`approval_use`）、期限與 `overdue`、已結束時的裁定（`action`、`option_id`、`note`、`batch_id`） |
| `POST …/decisions/{request_id}/answer` | `{action?, expected_revision, option_id?, note?, defer_minutes?}`；`action` 是 `accept`（預設）、`modify`、`answer`、`investigate`、`defer`、`reject`（AI-231）。200 回 `{decision, resumed_task_ids}`。同樣的處理重送是冪等的；不同的處理 409 `decision_already_answered`；內容在檢視後變動 409 `decision_revision_mismatch`（不寫入）；目標版本已變更 409 `decision_superseded`（過期標記會寫入）；已合併 409 `decision_merged`；這種請求不能用的處理方式 400 `decision_action_not_allowed`；未知選項 400 `decision_option_unknown`；找不到 404 `decision_not_found` |
| `POST /api/v1/decisions/batch` | `{items: [{workspace_id, request_id, expected_revision, action, option_id?, note?, defer_minutes?}]}`（1–50 項、同一請求只能出現一次）；每一項各自一個交易，回 `{batch_id, results}`，逐項 `ok` 或錯誤代碼。`batch_id` 只記在每筆裁定上供稽核，不形成概括授權（AI-232） |
| `POST …/decisions/{request_id}/merge` | `{into, expected_revision, into_expected_revision}`；把請求合併到另一筆（同種類、都待處理、不是權限提示，否則 400 `decision_merge_invalid`），阻擋範圍併入目標，回 `{decision}`（目標）（AI-233） |
| `POST …/decisions/{request_id}/links` | `{other_id}`；互相連結（冪等、雙向、不改內容版本）。Agent 提出請求時，同 Workspace 正規化後相同的待處理問題會自動連結 |
| `GET`／`PUT /api/v1/workspaces/{id}/decision-authority` | 決策權限表：`{revision, entries: [{matter, label, builtin, effective, narrowed}]}`；`PUT {expected_revision, entries: [{matter, effective}]}` |
| `GET /api/v1/decisions/pending` | 先標記過期的請求，再回 `{pending_count, by_workspace}`，給狀態列的「待裁定數」（延後中的仍計入） |

**版本校驗與過期**（AI-229、AI-230）：請求記下建立時的目標（目標 Task 與它的契約版本）。目標 Task 的契約版本改變或 Task 已結束，請求就標為 `superseded`，不能再套用；只被它阻擋的 Task 回到 `ready` 並重新排隊，喚醒 Outbox 帶 `superseded: true`。標記在開啟裁定中心、查詢待裁定數與提交時進行（不在修改契約的交易中連動）。權限提示記下判定當時的授權 revision 供查閱，但授權變更不會讓它過期（票 23：修改授權與回答請求是兩回事）。提交時必須帶使用者看到的 `revision`：延後、合併、過期、回答都會讓它加一，連結不會。

資料表（migration 15 建立、migration 22 重建）：`decision_request`（只有待處理時能變動，`revision` 只增不減；狀態 `pending`／`answered`／`superseded`／`merged`）、`decision_request_block`、`decision_request_link`、`decision`（追加式，記 `action` 與 `batch_id`）、`decision_approval_use`（追加式）、`decision_authority_policy`。結束請求（回答或過期）時追加的喚醒 Outbox 項目種類是 `decision_wake:<請求識別>`，被合併進來的請求的 Run 也各自得到一筆，**目前沒有消費者**（Controller 的事件迴圈在後續的票）。事件（聚合 `decision_request`，版本接續該請求最後一個事件）：`decision_requested`、`decision_answered`、`decision_deferred`、`decision_superseded`、`decision_merged`、`decision_merge_received`、`decision_linked`、`permission_approval_used`；自動允許的 `permission_auto_allowed`（每筆一個 `permission_audit` 聚合）；Controller 動作的 `controller_action_allowed`／`controller_action_refused`（每筆一個 `authority_audit` 聚合）；權限表變更 `decision_authority_changed`。

權限提示：Adapter 以 `--permission-prompt-tool mcp__baton__permission_prompt` 接上；回應形狀已以 Claude Code 2.1.307 實測（CLI 送來 `tool_name`、`input`、`tool_use_id`，接受帶 Baton 說明欄位的 `deny`，被拒絕的工具不執行、Worker 不重試同一個操作；見 `docs/adapter-contract.md`）。

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

結果未知期間，同一個鍵的登記一律回 `result_unknown`；別的 Task 用了同一個鍵也一樣，而且該 Task 被加入同一個請求的阻擋範圍，一起等查證。其他 Task 不受影響。使用者回答時，查證結果與 Task 恢復（回到 `ready` 並重新排隊）在同一個交易提交：`verified_done` → 操作 `succeeded`（回執記為「使用者查證已生效」加備註），之後同一個鍵回 `already_succeeded`；`verified_not_done` → 操作 `failed`，之後同一個鍵可以再執行一次（嘗試次數加一）。這種查證請求（裁定的 `external_operation_id` 指向操作）只能以「接受」其中一個選項或「延後」處理，其他方式回 400 `decision_action_not_allowed`；不能合併（400 `decision_merge_invalid`，可以改用連結），也不會因目標 Task 的契約或狀態改變而過期——否則請求結案了，操作卻一直停在 `unknown`。

桌面端點 `GET /api/v1/workspaces/{id}/external-operations` 回傳 `{ operations }`（依建立順序，含狀態、嘗試次數、回執、原因與裁定請求）。事件（`aggregate_type = 'external_operation'`）：`external_operation_begun`、`external_operation_succeeded`、`external_operation_failed`、`external_operation_result_unknown`、`external_operation_verified`；另一個 Run 的重複登記被擋下時追加 `external_operation_duplicate_blocked`（`aggregate_type = 'external_operation_duplicate'`，見〈事件時間線與觀測指標〉）。資料表（migration 25 `external_operation`）：`external_operation`（`(workspace_id, idempotency_key)` 唯一、不可刪除）與 `storage_probe`。

**寫入故障（磁碟滿、資料庫被鎖住）**：Store 的寫入佇列依 SQLite 錯誤代碼歸類每個失敗的寫入（`baton_store::storage_fault_of`）：`SQLITE_FULL` → `disk_full`、`SQLITE_BUSY`／`SQLITE_LOCKED` → `busy`、I/O 錯誤或唯讀 → `unwritable`；其他錯誤（例如約束違反）不算。有故障時：

- Supervisor 不啟動任何程序（第 1 步）、不交接（第 6 步）也不排程（第 7 步），每次處理先試寫 `storage_probe`；試寫成功才解除，沒有寫入任何東西的交易不算恢復。
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

進行中 = 尚未結束的 Run（`queued`／`starting`／`running`，含已要求取消但程序還沒確認停止者）。`SlotKind::Handoff` 可以使用保留槽，交接的 successor 就以它派出（見〈檢查點與主動交接〉）。選帳號：核對無誤、額度未達門檻、帳號名額未滿者中，進行中 Run 最少的；同數時取較早建立的帳號。

**等待原因**（`WaitReason`，依類別分開，讓使用者知道該處理哪一類問題）：

| `category` | 代表 | 例（`code`） | 怎麼恢復 |
| --- | --- | --- | --- |
| `capacity` | 名額已滿 | `account_capacity_full`、`global_capacity_full`、`handoff_slot_reserved` | 名額空出來自動派出 |
| `quota` | 帳號的額度門檻是 `paused`（判斷用量 ≥ 75%）或 `draining`（≥ 85% 或 CLI 限流警告，進行中的 Run 交接出去） | `quota_near_limit`、`quota_draining` | 用量回到 60% 以下、過了重置時間或用量變成未知，門檻回到 `open` 後自動派出（遲滯，見〈檢查點與主動交接〉）；用量未知**不**阻擋 |
| `authorization` | Workspace 授權不足或尚未確認 | `capability_not_granted`、`authorization_unconfirmed` | 使用者重新確認授權 |
| `dependency` | 相依的 Task 還沒完成 | `dependencies_unfinished` | 相依完成後自動派出 |
| `state_unknown` | 帳號還沒核對、無法確認身分 | `profile_unverified`、`identity_unconfirmed` | 在帳號頁核對 |
| `account` | 帳號不可用，或根本沒有帳號 | `no_account`、`profile_not_logged_in`、`auth_source_overridden`、`identity_mismatch`、`duplicate_identity` | 登入、移除覆蓋來源、新增帳號 |
| `guard` | 已啟用的 Guard 擋下（原因帶 `guard`：`guard_id`、`lesson_id`、`lesson_title`） | `guard_violated` | 依來源教訓修改契約，或停用該 Guard（停用後自動派出） |

等待原因每次由目前狀態重新判定，不保存（所以不會過期）。`GET /api/v1/scheduling` 回傳總覽：容量與使用量、每個使用中帳號的進行中 Run（`accounts[].runs`）、額度判斷（`accounts[].quota`、`quota_used`，未知為 `null`）、額度門檻（`accounts[].quota_gate`）與方案上限（`accounts[].plan_limits`）、佇列與各自的原因。排程只拿到已知用量的帳號（`baton_api::quota::known_used`）。

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

### 檢查點與主動交接

帳號額度接近上限時，Runtime 主動把進行中的工作交接到另一個帳號（issue §5.2 五步驟、spec D-13）。規則在 `baton_domain::handoff`（純邏輯：額度門檻的遲滯、交接 phase 狀態機、冷卻、安全邊界、successor 的 Prompt 段落），持久化在 `baton-store` 的 `handoffs.rs`（每個 phase 一個交易），端點在 `baton-api` 的 `agent.rs`（`confirm_handoff`）、`scheduling.rs`、`tasks.rs`，停止舊程序與轉移由 Supervisor 執行（`hand_off`、`stop_for_handoff`、`transfer_handoff`，Supervisor 的第 6 步）。

**額度門檻（遲滯）**：每個帳號一個門檻狀態（`quota_gate`，從未離開 `open` 的帳號沒有紀錄），依額度判斷的用量（較保守者，見〈額度觀測〉）推進：

| 狀態 | 進入條件 | 原因代碼 | 對排程的影響 |
| --- | --- | --- | --- |
| `open` | 用量回到 < 60%、用量變成未知（訊號已在重置時間失效），或已過進入時記下的重置時間；60%～75% 之間維持原狀 | `recovered`、`usage_unknown`、`reset_observed` | 可派工 |
| `paused` | 用量 ≥ 75% | `stop_dispatch_threshold` | 不派新工作，進行中的 Run 繼續；等待原因 `quota_near_limit` |
| `draining` | 用量 ≥ 85%，或目前有 CLI 的限流警告（或已達上限）；已在 draining 的帳號沒有過重置時間前，用量降到 75%～85% 也不退回 `paused` | `drain_threshold`、`limit_warning`、`limit_reached`（CLI 回報已達上限＝突然限流；已在 draining 的帳號撞到上限時原因也改成它） | 不派新工作，另把進行中的 Run 交接出去；等待原因 `quota_draining` |

門檻在 `baton_api::quota::current` 每次被呼叫時推進（Supervisor 每次處理、排程端點），狀態改變才寫入 `quota_gate`（記下原因、轉換當下的用量、進入 paused／draining 時的重置時間）並追加 `quota_gate_changed` 事件（`aggregate_type = 'quota_gate'`，payload 含 `from`／`to`／`reason`／`used`／`reset_at`）。

**交接 phase**（`handoff` 紀錄，`aggregate_type = 'handoff'`）：每一個 phase 持久化之後才做下一步的外部副作用。

| phase | 條件 | 持久化內容 | 之後才做的外部副作用 |
| --- | --- | --- | --- |
| `requested`（步驟 1） | 帳號 `draining`、Run 是 `running` 而且沒有要求取消、這個 Run 沒有進行中的交接、Task 不在冷卻期，而且找得到承接帳號（可派工、門檻 `open`、名額夠，以 `SlotKind::Handoff` 選，可用交接保留槽）；沒有承接帳號就不建立，Run 留在原帳號繼續執行 | 舊 Run、帳號與世代（`from_generation`）、選定的承接帳號、原因（門檻的原因代碼） | Worker 下一次回報的回應帶 `stop_requested`、`handoff_requested` |
| `stopping`（步驟 2） | 到了安全邊界，而且承接帳號仍有名額（可改用別的可派帳號）；否則留在 `requested` 下一次再判定 | 要延續的 checkpoint（Task 最新一筆被接受的 checkpoint，任一世代） | 要求舊程序停止（`ProcessControl::stop`，寬限期 `stop_grace`） |
| `transferred`（步驟 3、4） | 舊程序**確認停止**：`StoppedGracefully`／`ForceKilled`、`inspect` 為 `Dead`，或 Worker 在 `requested`／`stopping` 中自行結束（`stop_outcome` = `stopped_gracefully`／`force_killed`／`dead`／`exited`） | 單一交易：舊 Run `interrupted`（`handed_off`）、successor Run 與派工 Outbox、交接紀錄前進（`to_run_id`、`to_generation`、`stop_outcome`、`transferred_at`） | 一般的 Outbox 啟動 successor（同一次 `tick`） |
| `confirmed`（步驟 5） | successor Worker 呼叫 `confirm_handoff` | 確認內容（已完成、未完成、未知副作用）與 `confirmed_at` | 無 |
| `abandoned` | 舊 Run 已由別的路徑結束（取消、程序消失、完全退出；原因 `from_run_ended：…`）、舊 Run 已不持有 `from_generation` 的 lease（`generation_changed`），或轉移時沒有承接帳號（`no_successor：…`）；等待資源時 Task 已不在 `waiting_resource`（例如被取消，`task_left_waiting_resource：…`） | `abandoned_reason` | 沒有承接帳號時 Task 已在同一個交易排回派工佇列，由一般排程接手 |
| `waiting_resource`（票 29） | 突然限流的交接在轉移時，所有合適帳號都受限（名額以舊 Run 結束後計） | 單一交易：舊 Run `interrupted`（`handed_off_waiting_resource`，`RunCommand::ParkForResource`）並釋放 lease、Task 進入 `waiting_resource`、要延續的 checkpoint、`waiting_at`、`next_check_at`、`wait_reset_at`、`wait_attempts`、`wait_reasons` | 到了 `next_check_at` 才重新評估：有承接帳號就以單一交易讓 Task 回到 `ready`、派出 successor（Prompt 附上最近的有效 checkpoint）並轉移（`transferred`）；仍受限就重新排定（事件 `handoff_wait_rescheduled`） |

- **不會有兩個 writer**：舊程序確認停止之前不建立 successor。`stop` 回 `StillAlive` 或停止失敗時交接留在 `stopping`、舊 Run 繼續持有寫入所有權，下一次再試；完全退出開始時交給停止流程處理。
- **轉移是單一交易**（`Store::transfer_handoff`）：條件更新檢查舊 Run 仍持有 `from_generation` 的 lease → `RunCommand::HandOff` 結束舊 Run（`interrupted`、`end_reason` = `handed_off`、事件 `run_handed_off`）並釋放 lease、Task 回到 `ready` → 以 `SlotKind::Handoff` 選承接帳號（優先選定的那個）並 `RunCommand::Dispatch` 派出 successor（完整的派工檢查）→ successor 的 Prompt 附上 `handoff::successor_prompt` 的交接段落（舊 Run 與世代、checkpoint 摘要與細節，沒有 checkpoint 時要求從工作目錄判斷，並要求先核對再呼叫 `confirm_handoff`）→ 交接紀錄前進。已轉移過時原樣回傳（冪等）。
- **安全邊界**（`at_safe_boundary`）：舊 Run 在要求之後保存了 checkpoint，或等滿 `CHECKPOINT_WAIT_MS`（2 分鐘）。Worker 透過回報回應的 `stop_requested`／`handoff_requested` 得知要保存 checkpoint 並結束；它自行結束就直接轉移，不必等到 `stopping`。
- **冷卻**：同一個 Task 兩次換帳號至少間隔 10 分鐘（`HANDOFF_COOLDOWN_MS`，從上一次的 `transferred_at` 起算；放棄的交接不算）。突然限流不受冷卻限制（spec D-13）。
- **突然限流**（票 29，門檻原因 `limit_reached`）：舊 Worker 已無法呼叫模型，所以交接不等 checkpoint（安全邊界立即成立，以最近的有效 checkpoint 為準，不依賴最後一輪摘要）、不需要先有承接帳號（`to_profile_id` 可以是空字串）。CLI 仍在等待額度時 Supervisor 查證並停止它；CLI 自行以失敗結束時，Supervisor 在收事件時以 `Store::request_sudden_handoff` 建立交接並直接轉移（判斷依據是持久化的門檻，不是記憶體中的旗標）。successor 的 Prompt 說明舊 Worker 沒有最後一輪的進度或摘要。
- **全部受限時等待資源**：重新評估的時間由 `handoff::next_resource_check` 決定——每個合適帳號不能接手時的重置時間（門檻關閉的帳號取門檻記下的 `reset_at`，其餘如名額已滿為未知）中，最早而尚未過去者之後 1 分鐘；有任何未知（或根本沒有可用帳號）時另算退避 `5 分鐘 × 2^wait_attempts`、上限 60 分鐘，兩者取較早者。`Store::resume_resource_waits` 在 Supervisor 的第 6 步處理：停止意圖生效或 Workspace 等待「繼續工作」時不派工也不重新排定（資源恢復不會解除完全退出；解除後的下一次處理就重新評估）；派工檢查沒過（Guard、工作目錄）時什麼都不寫、以那個原因重新排定。等待資源不計入任何預算。
- **交接不是失敗**：舊 Run 以 `handed_off` 結束，不計入任何修正或重試預算（spec D-16）。
- **一次只有一個**：一個 Run、一個 Task 同時最多一個進行中（`requested`、`stopping`）的交接；一個 Task 也最多一個等待資源的交接（Domain 的候選判定排除已在交接中的 Run，資料庫另有唯一索引 `handoff_one_in_flight_per_run`、`handoff_one_in_flight_per_task`）。
- **不交接的時候**：控制資料庫無法寫入時整個第 6 步暫停；完全退出的停止意圖生效或 Workspace 等待「繼續工作」時找不到承接帳號（交接也是派工，經過 `Capacity.halt`）；崩潰恢復擋住派工時（恢復事件 `waiting`、`attempting`、`exhausted`）Supervisor 略過整個第 6 步，舊程序交給恢復嘗試停止。
- **崩潰恢復**：外部副作用都在 phase 持久化之後，Runtime 重建後依 phase 繼續：`requested` 重新判定；`stopping` 先 `inspect`（已經 `Dead` 就不再要求停止）再要求停止；`transferred` 的 successor 走一般的啟動 Outbox，以 `command_id` 找回程序、不重複啟動；`waiting_resource` 保留原本排定的 `next_check_at`。

| 端點／工具 | 用途 |
| --- | --- |
| `confirm_handoff`（Agent 工具） | 本文 `{ "completed", "incomplete", "unknown_side_effects" }`，三欄必填（去掉前後空白；沒有就寫「無」）、各 4000 字元上限（`CONFIRMATION_MAX_CHARS`）；回 `{ "confirmed": true, "handoff" }`，重複確認是冪等的。400 `handoff_confirmation_empty`、`handoff_confirmation_too_long`；不是等待確認的 successor 409 `handoff_not_pending`；已不是寫入所有者 409 `run_report_stale` |
| `GET /api/v1/scheduling` | `accounts[].quota_gate`：帳號的門檻（`state`、`reason`、`used`、`reset_at`、`since`、`version`；從未離開 `open` 為 `null`） |
| `GET /api/v1/workspaces/{id}/tasks` | 工作面板多了 `handoffs`：Workspace 內全部交接，依建立順序（phase、舊／新 Run 與帳號、世代、checkpoint、停止結果、確認內容、放棄原因與各 phase 的時間；等待資源時另有 `waiting_at`、預計重新評估的 `next_check_at`、`wait_attempts`、`wait_reset_at`（未知為 `null`）與 `wait_reasons`）。Web UI 的工作卡片在等待資源時顯示預計重評時間、依據與等待原因 |

事件：`quota_gate_changed`、`handoff_requested`、`handoff_stopping`、`handoff_transferred`、`handoff_confirmed`、`handoff_abandoned`、`handoff_waiting_resource`、`handoff_wait_rescheduled`，以及 Run 的 `run_handed_off`（等待資源時 `end_reason` 是 `handed_off_waiting_resource`）。資料表（migration 29 `quota_gate_and_handoff`，migration 32 `handoff_waiting_resource` 重建 `handoff` 加入 `waiting_resource` 與等待欄位）：`quota_gate`（每個帳號一列）與 `handoff`（`to_run_id` 只在 `transferred`／`confirmed` 有值、`waiting_resource` 一定有 `next_check_at`，以 CHECK 保證）。

測試：`crates/baton-harness/tests/handoff.rs`、`crates/baton-harness/tests/sudden_limit.rs`（情境測試）、`crates/baton-domain/src/handoff/tests.rs`。

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
| `command` | `program`、`args`、`expected_exit_code`（預設 0） | 在受驗 Run 的工作目錄執行（不經過 shell，`CommandRunner` Port），結束碼相符；結束碼不符算未通過，逾時 10 分鐘、被終止或無法啟動取不到證據，算**不完整**（`inconclusive`，票 36） |
| `file` | `path`、`sha256?` | 工作目錄內的相對路徑（不可含 `..`、磁碟代號、從根目錄開始）是存在的檔案，有 `sha256` 時雜湊相符（`WorkspaceFs::read_workspace_file`）；讀取失敗算不完整 |
| `artifact` | `name`、`sha256?` | 這個 Task 有這個名稱的成果，最新版本的檔案與 manifest 雜湊一致，有 `sha256` 時也相符 |

**流程**：Supervisor 每次處理的最後一步（第 8 步）找出 `verifying`、有受驗 Run（世代最大且 `succeeded` 的 Run）而且契約宣告了檢查的 Task，逐項取得事實（命令輸出保留結尾 4000 字元作為證據）、由 Domain 判定，再以**成果接受交易**提交。全部通過 → `completed`；任一未通過 → 回到 `ready`（可修正、可再派工；不會自動重新排隊），證據保存在驗證紀錄；沒有未通過但有不完整 → 結果 `inconclusive`，Task 留在 `verifying`，下一次處理重新執行檢查（重驗）。未通過與不完整分別計入修正與重試預算（見「修正與重試預算」）。沒有宣告任何檢查的 Task 停在 `verifying`，等使用者手動接受。背景迴圈把這一步放在另一個工作執行，長時間的檢查命令不會擋住啟動、取消與 lease 核對；測試 harness 的 `supervise()` 則會等它做完。Runtime 在檢查途中當掉時沒有任何紀錄，重啟後會重新執行檢查。

**成果接受交易**同時校驗三件事，任一不符就拒絕、什麼都不寫：

| 校驗 | 不符時 |
| --- | --- |
| 受驗 Run（`run_id`＋`generation`）仍是這個 Task 世代最大、已成功結束的 Run | 409 `acceptance_run_stale` |
| 契約版本與檢視時相同，且等於 Run 派工時的契約版本 | 409 `acceptance_contract_stale` |
| 成果 manifest（每個成果的最新版本）與檢視時相同 | 409 `acceptance_manifest_stale` |

契約在 Run 派工後被改版時，Supervisor 不執行檢查，直接記錄一筆「未通過」（原因寫在 `note`），Task 回到 `ready` 重新執行；這個 Run 的成果不能在新契約下被接受，手動接受也會被拒絕。過期 Run 的回報（登記成果、結束事件）本來就因不是目前寫入所有者而被拒絕（`run_report_stale`），所以不能推進 Task。Workspace 封存後，其中 `verifying` 的 Task 不再自動檢查（`automatic` 為 `false`），成果接受（含手動接受）一律 409 `workspace_archived`，Task 停在 `verifying`。

**驗證紀錄**：追加式資料表 `task_verification`（migration 16 建立、migration 27 重建，觸發器禁止更新與刪除；部分唯一索引：一個 Run 最多一筆有結論（`passed`／`failed`）的紀錄、`inconclusive` 可以有多筆，一個 Task 最多一筆 `passed`）。每筆含方式（`checks`／`manual`）、結果（`passed`／`failed`／`inconclusive`）、受驗 Run 與世代、契約版本、每項檢查的結果與證據、驗證時的成果 manifest、手動接受者（桌面憑證雜湊的前 16 字元，**不是**憑證本身）與附註。同一個交易追加 Task 事件 `task_verification_recorded`（payload 含上述摘要與未通過的檢查）與 `task_status_changed`（`reason` 為 `verification_passed`／`verification_failed`）。

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

### 修正與重試預算

每個 Task 有三種互不影響的計數（spec D-16，規則在 `baton_domain::budget`，計入點在 `run`、`verification` 的轉換中）：

| 種類（`kind`） | 上限 | 計入的事件 |
| --- | --- | --- |
| 實作失敗修正（`implementation`） | 3 | Worker 以失敗結束（`run_failed`，`exited_with_failure`）；確定性檢查未通過 |
| 驗證不完整重驗（`verification`） | 2 | 確定性檢查結果 `inconclusive`（取不到證據） |
| 基礎設施重試（`infrastructure`） | 5 | 啟動失敗（`launch_failed`）；lease 過期後確認程序已不在（`process_dead_after_lease_expired`） |

**不計入任何一種**：突然限流（Adapter 回報額度用盡 `RateLimited { status: exhausted }` 之後程序以失敗結束 → 交接給其他帳號或等待資源，見〈檢查點與主動交接〉；建立交接失敗時退回 `RunOutcome::QuotaLimited`：Run `interrupted`，`end_reason` 為 `quota_limited：…`，Task 回到 `ready`）、恢復與完全退出的中斷（`Interrupt`：`launch_outcome_unknown`、完全退出、還原）、取消，以及「契約在派工後變更」造成的未通過。

**預算內**：計數加一，Task 照原本的轉換（實作失敗回 `ready`、不完整留在 `verifying`）。基礎設施重試另外自動排回派工佇列，並設定退避期限 `retry_not_before`（1、2、4、8、10 分鐘，上限 10 分鐘）；期限前排程的等待原因是 `backoff`／`retry_backoff`。實作失敗不會自動重新排隊（由 Controller 在預算內重派或使用者派工）。

**用盡**（已用到上限後再發生一次）：計數停在上限，Task 進入 `blocked`，同一個交易建立「預算用盡後的下一步」裁定請求（`kind = authority`、`authority.matter = next_step_after_budget`，識別為 `dr_budget_<Run 或驗證紀錄識別>`，阻擋範圍是這個 Task，選項 `approve`／`reject`）。核准 `approve` → 只重設用盡的那一種計數，Task 繼續（驗證類回到 `verifying` 重驗，其他回到 `ready` 並排回派工佇列），核准同時記為已使用（Controller 不能再拿它當單次核准）；駁回 → Task 維持 `blocked`，由使用者修改契約、取消或手動標為可派工（`POST ready`）。

資料表 `task_budget`（migration 27 `retry_budgets`；從未計入的 Task 沒有列）。事件聚合 `task_budget`（聚合識別是 Task）：`task_budget_charged`、`task_budget_exhausted`、`task_budget_reset`。`GET tasks/{task_id}` 回傳 `budget`、`GET tasks` 回傳 `budgets`（`BudgetView`：三項 `counters`（`kind`、`label`、`used`、`limit`）、`retry_not_before`、`exhausted`、`decision_request_id`）；Web UI 在 Task 卡片顯示計數、退避期限與用盡提示。

突然限流的判定以持久化的額度門檻（`limit_reached`）為準（票 29），Runtime 在「回報額度用盡」與「程序結束」之間重建也一樣；Supervisor 記憶體中的旗標只在建立交接失敗時作為退路。

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

### 事件時間線與觀測指標

「時間線」分頁（`web/src/TimelinePage.tsx`，側欄的「時間線」）呈現 Workspace 內派工、交接、裁定、恢復、驗證與清理事件，可依 Task 過濾；上方是由同一份事件計算的觀測指標（spec 使用者故事 92、93）。分類在 `baton_domain::timeline`，指標在 `baton_domain::metrics`（純函式），讀取在 `baton-store` 的 `timeline.rs`，端點在 `baton-api` 的 `timeline.rs`。沒有新的資料表：時間線直接讀追加式的 `event` 表。

**時間線上的事件**（`category`）：

| `category` | 事件 |
| --- | --- |
| `dispatch` | Run 的 `run_created`、`run_admission_denied`、`run_launch_started`、`run_started`、`run_succeeded`、`run_failed`、`run_interrupted`、`run_cancel_requested`、`run_cancelled`（進度、checkpoint、lease 續約、原生 Session 綁定不上時間線） |
| `handoff` | `handoff` 聚合的全部事件與 Run 的 `run_handed_off` |
| `decision` | `decision_request` 聚合的 `decision_requested`、`decision_answered`、`decision_deferred`、`decision_superseded`、`decision_merged`、`decision_merge_received`、`decision_linked` |
| `recovery` | `recovery_event` 聚合的全部事件 |
| `verification` | `task_verification_recorded` |
| `cleanup` | `worktree_cleanup` 聚合的全部事件 |
| `task`、`external_operation`、`lesson`、`usage` | 指標用到的事件：`task_created`／`task_status_changed`；外部操作的全部事件（含 `external_operation_duplicate_blocked`）；`lesson_recurrence_recorded`；`run_usage_reported` |

**Task 歸屬**：Run、交接、裁定請求依各自資料表的 `task_id`；Task 與清理的聚合識別就是 Task；外部操作、重犯、用量依事件內容的 `task_id`／`run_id`（外部操作重試時操作紀錄的 `task_id` 會換成新的 Task，所以以事件當時的 Run 為準）。恢復事件屬於整個 Runtime，出現在每個 Workspace 的時間線上，`task_ids` 是它任何一次嘗試處置過、屬於這個 Workspace 的 Task。不屬於任何 Task 的事件依它記下的 Workspace 歸屬：沒有指定 Task 的重犯、Controller Run 的用量（票 16；計入 Workspace 的用量指標，`task_ids` 為空）。別的 Workspace 的事件不會出現。依 Task 過濾 = 未過濾的時間線中 `task_ids` 含這個 Task 的事件，順序不變。

**新事件**：
- `run_usage_reported`（`aggregate_type = 'run_usage'`，`aggregate_id` = Run，版本是這個 Run 第幾次回報）：每次記下 Run 的用量（`record_quota_usage`）時在同一個交易追加，內容 `{ run_id, task_id, profile_id, tokens, context_tokens }`。`quota_usage` 會在 8 天後刪除，指標改由這個不可刪除的事件計算。`context_tokens` 由 Adapter 的 `AdapterEvent::UsageReported.context_tokens` 提供（真實 Adapter：`result.usage` 的輸入 + 快取建立 + 快取讀取；沒有這些欄位為 `null`）。
- `external_operation_duplicate_blocked`（`aggregate_type = 'external_operation_duplicate'`，每次擋下各是一個聚合，識別是這次登記新配的操作識別，不改動原操作的版本）：另一個 Run 以同一個冪等鍵登記，回 `already_succeeded`、`result_unknown` 或 `in_progress_elsewhere` 時追加，內容含原操作、冪等鍵、`status`、Run 與 Task。同一個 Run 重送登記（回應遺失）不算。

**指標**（`metrics::compute`，沒有資料時比率與平均是 `null`，UI 顯示「無資料」，不填零）：

| 指標 | 定義 |
| --- | --- |
| 工作完成率 | 最後狀態為 `completed` 的 Task ÷ 建立的 Task（不含已取消） |
| 交接成功率 | 到達 `handoff_transferred` 的交接 ÷ 已結束（轉移或放棄）的交接；另列確認接手數與進行中數 |
| 恢復耗時 | `runtime_crash_detected` → 最後成功的那次嘗試的 `recovery_attempt_completed`（工作重新排隊），不含 10 分鐘穩定觀察；尚未成功的恢復事件算在 `pending` |
| 重複操作事件 | `external_operation_duplicate_blocked` 的次數，依擋下時的狀態分開；另列變成結果未知的次數 |
| 裁定等待時間 | `decision_requested` → 第一次 `decision_answered`；未回答的算在 `pending`，被取代或合併的不計 |
| 教訓重犯率 | 有 `lesson_recurrence_recorded` 的 Task ÷ 建立的 Task；另列攔下與回報的次數 |
| 每階段用量 | `run_usage_reported` 的 token 依 Stage 加總；第一版沒有 Workflow／Stage，全部歸在「未分階段」（`stage: null`） |
| Context 使用量 | 每個 Run 的 `context_tokens` 加總後的平均與最大；沒有回報 Context 的 Run 另計 `runs_unknown` |

| 端點（桌面憑證） | 用途 |
| --- | --- |
| `GET /api/v1/workspaces/{id}/timeline?task_id=` | `{ workspace_id, task_id, events }`，事件依 `seq`（事件表的提交序號）遞增：`seq`、`occurred_at`（UTC 毫秒）、`category`、`aggregate_type`、`aggregate_id`、`event_type`、`task_ids`、`run_id`、`payload`。Workspace 不存在 404 `workspace_not_found`；Task 不屬於這個 Workspace 404 `task_not_found` |
| `GET /api/v1/workspaces/{id}/metrics` | `{ workspace_id, metrics, server_quota }`：`metrics` 是 `metrics::compute(未過濾的時間線)`，所以拿時間線重新計算一定得到相同的數字；`server_quota` 是每個使用中帳號的服務端額度（`five_hour_used`、`seven_day_used`），只採用 Claude Code 回報的帳號層級用量或外部觀測器的已觀測樣本，本機推估不算；看不到為 `null`，UI 顯示「未知」 |

桌面殼指令：`workspace_timeline`（`workspaceId`、`taskId`）、`workspace_metrics`（`workspaceId`），經 `RuntimeClient::timeline`／`metrics`。

測試：`crates/baton-harness/tests/timeline_and_metrics.rs`（情境測試）、`crates/baton-domain/src/metrics/tests.rs`、`crates/baton-client/tests/timeline_client.rs`、`web/src/TimelinePage.test.tsx`。

### Runtime 意外崩潰後的自動恢復

依 ADR-0005（DR-20261021-05）。規則在 `baton_domain::recovery`，持久化在 `baton_store` 的 `recovery.rs`，執行在 Supervisor。

**判定崩潰**：`RunningRuntime::shutdown()` 是有秩序的停止，會在 `runtime_boot` 記下 `ended_cleanly_at`；`RunningRuntime::kill()`、程序被終止或當掉都不會。Runtime 啟動時（停止意圖已核對之後、任何程序副作用之前）若前一次啟動沒有正常結束、也沒有停止意圖，就是意外終止：沒有未結束的恢復事件時建立一個（第 1 輪、尚未嘗試），有的話沿用。完全退出的停止意圖優先，那種情況走「重新開啟」，不是崩潰恢復。

**一次嘗試**（Supervisor 的 `tick` 在任何啟動之前處理）：
1. 先持久化嘗試（輪次、序號 1～3、嘗試識別）；寫不進去就不開始，不碰任何程序。資料表以 `CHECK (seq BETWEEN 1 AND 3)` 與 `UNIQUE (event_id, round, seq)` 保證同一輪不可能有第四次。
2. 盤點並核對：以 command_id 找回啟動中（回執未寫回）的程序並補寫回執，確定沒有程序的照舊以 `launch_outcome_unknown` 中斷、不自動重派；收下已到達的 Adapter 事件（保存證據，已自行結束的 Run 正常結束）。
3. 對每個尚未結束、身分已確認的舊程序並行呼叫 `ProcessControl::stop(identity, 30 秒)`。確認停止的 Run 以 `interrupted`（`stopped_by_crash_recovery`）結束、釋放寫入所有權，Task 退回 `ready` 並重新排隊，由排程以**新 Run** 恢復——新 Run 照常經過准入（授權、共同設定版本）、相依與 Guard 檢查，原生 Session 依票 15 的規則決定沿用或替換。崩潰前已要求取消的 Run 則完成取消，不恢復；被暫停的 Workspace 照舊等待。
4. 全部確認停止：進入穩定觀察（`observing`），10 分鐘後持久化成功（`succeeded`）。任何一個程序無法確認停止（`StillAlive`、停止失敗、或啟動中而無法查證）：這次嘗試失敗，那些 Run 保留寫入所有權、不建立新 Run。

**失敗、退避與用盡**：嘗試失敗或嘗試中／觀察中再次崩潰，都沿用原事件與已用次數；還有額度就等待退避（第二次前 30 秒、第三次前 2 分鐘，從失敗被記下的時刻算起，期限持久化在 `next_attempt_at`），退避期間重建 Runtime 不計次也不改期限。第三次失敗後進入 `exhausted`：停止自動恢復，`halt_for`／`launch_halted_in` 擋住所有派工與啟動（等待原因 `lifecycle`／`crash_recovery_exhausted`）；重建、等待都不清除。使用者在恢復面板按「重新嘗試恢復」才建立新輪次（舊輪次的嘗試保留為歷史）。等待或進行嘗試期間同樣不派工（`crash_recovery_pending`）；穩定觀察期間可以派工。恢復次數與 Task 的修正預算、額度交接互不相干（D-16）。

| 端點（桌面憑證） | 用途 |
| --- | --- |
| `GET /api/v1/recovery` | 最新的恢復事件（狀態、輪次、本輪已用次數、退避期限、觀察期滿時間、最後失敗原因、每次嘗試與受影響工作）、`max_attempts`、`can_retry`、`blocking`（派工阻擋原因） |
| `POST /api/v1/recovery/retry` | 重新嘗試恢復；只在 `exhausted` 時接受，否則 409 `recovery_not_exhausted`（沒有事件時 `no_recovery_event`） |

事件：`recovery_event` 聚合有 `runtime_crash_detected`、`recovery_attempt_started`、`recovery_attempt_completed`、`recovery_attempt_failed`、`recovery_succeeded`、`recovery_exhausted`、`recovery_retry_requested`。Web UI：恢復面板（`RecoveryPanel`）與頂部狀態列的恢復事件狀態。

自動重新啟動 Runtime 的是監護程序（下一節）；沒有監護時，要等使用者（或 App）再次啟動 Runtime 才開始恢復。

**Validation Required**：30 秒、2 分鐘、10 分鐘是產品政策值；Windows 上 Runtime 被終止時舊 Worker 的實際存活情形、受控停止在真實 Claude Code 上的效果需手動確認。

### 監護程序與自動重啟

依 ADR-0006（DR-20261021-06）與 spec D-24。規則在 `baton_domain::guardian`（純邏輯）與 `baton_domain::recovery::import_guardian_attempt`，監護本身是 `crates/baton-guardian`（`baton-guardian.exe`），匯入在 `baton_store` 的 `recovery.rs`（migration 31：`guardian_journal_import`）。

**誰啟動監護**：桌面 App 連上（或啟動）Runtime 之後，看 `GET /api/v1/health` 的 `guardian`：不是 `active` 就啟動 `baton-guardian.exe`（`baton_client::ensure_guardian`）。所以重開 App 時只補缺少的元件：Runtime 還在就不重啟它，也不重設任何次數。監護以具名 mutex 保證同一個資料目錄只有一個；啟動時沒有執行中的 Runtime 就直接結束（監護只重啟它親眼看到結束的 Runtime）。Runtime 不會反向啟動監護。

**監看與確認終止**：監護每秒查詢一次 Runtime 程序是否還在（自己啟動的看子程序，App 啟動的看 Runtime 寫下的 `guardian\runtime.json`：程序識別碼與記錄時間，程序建立時間晚於記錄時間就是識別碼被重複使用）。只有確認程序已經結束才算終止，不以 heartbeat 或 API 逾時推定。

**重啟前**（每一步都在監護程序內，不開控制資料庫）：
1. 讀生命週期檔：有完全退出的停止意圖、維護抑制（`maintenance`：更新、解除安裝，由票 34 寫入）、或檔案讀不懂（停止意圖不明）就不啟動，監護結束。
2. 以 Runtime 寫的恢復狀態鏡像（`guardian\recovery.json`：最新事件的狀態、輪次、已用次數、退避期限，以及已匯入的最後一筆監護日誌 `journal_cursor`）加上游標之後的監護日誌，算出這是第幾次：沒有未結束的事件就是新事件的第 1 次；嘗試中或觀察中終止的那一次算失敗，退避（30 秒、2 分鐘）從監護確認終止的時間起算；Runtime 自己的退避期限照舊；三次用盡就不啟動，監護結束，等使用者開 App。
3. 把「事件、輪次、序號、嘗試識別、時間、原因」以一行 JSON append 到監護日誌（`guardian\journal.jsonl`）並 fsync；寫不進去就不啟動，監護結束。
4. 最後核對：這時已經有 Runtime（例如使用者剛開 App）就寫下撤回紀錄並改為監看它；生命週期檔剛出現抑制就寫下撤回紀錄並結束。撤回的嘗試不計次。
5. 以環境變數 `BATON_GUARDIAN_ATTEMPT=<嘗試識別>` 啟動 `baton-runtime.exe`。程序啟動後很快就結束（啟動早期失敗）也已經計次。

**Runtime 啟動時**：停止意圖核對之後、崩潰判定與任何恢復副作用之前，先匯入監護日誌：已匯入的嘗試識別略過；撤回的記為 `withdrawn`；停止意圖生效時記為 `ignored_stop_intent`（完全退出優先）；這一輪已用盡記為 `ignored_exhausted`（不開始第四次）；其餘計入恢復事件（`counted`）——當時進行中或觀察中的嘗試記為失敗，以監護的嘗試識別開始下一次，序號以控制資料庫為準，不再檢查退避（監護已等過）。接著：
- 監護拉起的 Runtime 若停止意圖仍生效：不提供服務、記下正常結束後結束（`RuntimeError::SuppressedByStopIntent`），並把停止意圖補進生命週期檔，監護下一次就看到抑制。等使用者手動開 App 才依完全退出後的規則重新開啟。
- 監護拉起的 Runtime 接續那次嘗試（不記為失敗）；Supervisor 第一次 tick 看到嘗試中的事件就執行盤點、受控停止舊程序與以新 Run 恢復（與票 31 的一次嘗試相同）。手動開 App 啟動的 Runtime 不接手監護的嘗試，照票 31 把它記為失敗。
- 之後每次恢復事件有變動（含使用者「重新嘗試恢復」），Supervisor 都重寫恢復狀態鏡像。

**降級**：Runtime 以監護寫下的 `guardian\guardian.json`（程序身分）判斷監護是否還在，`GET /api/v1/health` 的 `guardian` 是 `{ "state": "active" | "unavailable", "message": … }`（舊版 Runtime 沒有這個欄位，視為 `unknown`）。監護不在時工作照常，頂部狀態列顯示「工作繼續，自動重啟保護不可用」。監護與 Runtime 都不在時，等使用者開 App。

| 檔案（`<資料目錄>\guardian\`） | 寫入者 | 內容 |
| --- | --- | --- |
| `journal.jsonl` | 監護（append + fsync） | `attempt`（事件、輪次、序號、嘗試識別、確認終止時間、寫下時間、原因）與 `withdrawn`（撤回的嘗試識別與原因） |
| `recovery.json` | Runtime（原子寫入） | 最新恢復事件的摘要與 `journal_cursor` |
| `runtime.json`／`guardian.json` | Runtime／監護 | 程序識別碼與記錄時間 |
| `guardian.log` | 監護 | 診斷文字，不參與任何判斷 |

**Validation Required**：監護在 Windows 上偵測 Runtime 結束、以 `baton-guardian.exe` 拉起 `baton-runtime.exe`、登出時監護與 Runtime 一起結束，以及每秒輪詢的負擔，都只有單元測試與 harness 情境測試，需在 Windows 上手動確認（工作管理員結束 `baton-runtime.exe` → 監護重新啟動它 → 恢復面板出現同一次嘗試；結束 `baton-guardian.exe` → 狀態列顯示降級 → 重開 App 只補監護）。

### 跨 Windows 登入的啟動與續跑

依 ADR-0007（DR-20261021-07）與 spec D-25。規則在 `baton_domain::login`（純邏輯），持久化在 `baton-store` 的 `lifecycle.rs`（migration 36：`runtime_boot.login_session`、`workspace_lifecycle.auto_resume_across_login`／`paused_by`、`login_startup`），啟動入口在 `baton-guardian` 的 `launcher.rs`，Windows 實作在 `baton-runtime` 的 `windows_login.rs`。

**兩項設定，都預設關閉**，與 DR-01 的「完全退出後自動續跑」分開保存與顯示，變更都不會立即啟停目前的工作：
- 「登入 Windows 時自動啟動 Baton」：目前使用者層級的偏好。開啟時 Runtime 在 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 登記值 `Baton` = `"<安裝資料夾>\baton-guardian.exe" --login --data-dir "<資料目錄>" --port <埠>`；先改登記、成功才保存偏好。Runtime 每次啟動都讓登記跟偏好一致（被刪掉就補回）。完全退出不改變偏好也不移除登記（停止抑制另存於生命週期檔）。
- 各 Workspace 的「跨登入自動續跑」：只決定跨了登入的那次啟動後，這個 Workspace 要不要續跑。

**登入啟動入口**（`baton-guardian.exe --login`）：先讀 `lifecycle.json`，有完全退出的停止意圖、維護抑制或檔案讀不懂就直接結束（不啟動 Runtime、監護或任何工作）。否則沒有 Runtime 時以 `BATON_START_SOURCE=login` 啟動 `baton-runtime.exe`，再接著當監護。登入啟動的 Runtime 若在資料庫看到停止意圖（生命週期檔沒寫成的情形），與監護拉起的一樣不提供服務並把停止意圖補回檔案。完全退出後只有使用者手動開 App 才依 DR-01 重新開啟。

**跨登入的判斷**：每次啟動都在 `runtime_boot.login_session` 記下目前的登入識別（Windows：登入時間 + 終端機工作階段，不只用會在重開機後重複的 LUID），與上一次啟動比較。不論是登入啟動還是手動開 App 啟動都同樣判斷，所以兩者同時發生時結果相同；多出來的 Runtime 因單實例鎖在開資料庫前就結束，只有一個控制平面。停止意圖核對之後、任何程序副作用之前：
- 跨了登入（`new_login`）：沒有開啟跨登入自動續跑的使用中 Workspace 暫停（`paused_by = new_login`），等「繼續工作」；開啟的照常續跑。
- 取不到登入識別（`login_unverified`）：無法核對，每個使用中的 Workspace 都暫停。
- 同一次登入或第一次啟動：不做任何事。停止意圖生效時也不做任何事。
- 只會新增暫停，不解除既有的暫停；完全退出後重新開啟時，「完全退出後自動續跑」只解除完全退出造成的暫停。恢復事件、次數、退避與額度阻擋都不因重新登入而重設。

| 方法與路徑 | 說明 |
| --- | --- |
| `GET /api/v1/lifecycle` | 另含 `login_startup`（`enabled`、`registered`：Windows 是否已登記，查不到為 `null`），各 Workspace 另含 `auto_resume_across_login` 與 `paused_by`（`full_exit`／`new_login`／`login_unverified`） |
| `PUT /api/v1/lifecycle/login-startup` | `{ "enabled": bool }`；登記失敗 500 `login_startup_registration_failed`，偏好不變 |
| `PUT /api/v1/workspaces/{id}/login-resume` | `{ "auto_resume_across_login": bool }`；找不到 Workspace 404 |

事件：`login_startup` 聚合有 `login_startup_changed`；`workspace_lifecycle` 聚合另有 `workspace_login_resume_changed`、`workspace_paused_by_login`。

**Validation Required**：HKCU Run 在真實 Windows 登入時是否執行啟動入口、登入識別在登出再登入與重新開機後是否改變、以系統管理員提升權限執行時是否被誤判為跨登入，都只有單元測試、harness 情境測試與真實程序測試，需在 Windows 上手動確認（開啟登入時自動啟動 → 登出再登入 → Runtime 與監護已在執行、沒開跨登入自動續跑的 Workspace 等「繼續工作」；完全退出 → 重新開機 → 登入後沒有任何 Baton 程序）。

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

**已知限制**：Guard 只在 Run 准入時檢查，不攔截執行中的工具呼叫（權限提示仍依 Workspace 授權判定）；把教訓載入 Prompt 見〈Context Builder 與 context receipt〉。

### Context Builder 與 context receipt

每個 Worker Run 的 Prompt 由 Context Builder 在**派工的同一個交易**中組裝，並保存 context receipt（spec D-07、D-27、使用者故事 61）。規則在 `baton_domain::context`（純邏輯），輸入的取得與 receipt 的保存在 `baton-store` 的 `context.rs`，端點在 `baton-api` 的 `runs.rs`，UI 在工作面板 Run 狀態列的「Context receipt」。四個派工點都經過它：`apply_run_command` 的 `Dispatch`、`Store::schedule`、交接轉移與資源恢復（後兩者帶入交接）。Controller 的輪次不是 Run，不產生 receipt。

**內容與優先序**（由前到後；Prompt 也依這個順序排列）：

1. Task 契約（`run` 模組產生的標題、六項契約與確定性檢查）——必要。
2. 交接說明（successor Run；內含要延續的檢查點與 `confirm_handoff` 指示）——必要。沒有交接時改為「最近的檢查點」（Task 最近一筆被接受的 `save_checkpoint`，任一世代），依預算放入。
3. 適用決策、4. 相關教訓：只取**有效且適用**的條目（已驗證、在有效期間內、範圍是全域或涵蓋這個 Task 的 Workspace／Goal／Task；一般知識 `fact` 不載入，用 `search_knowledge` 查）。各自依相關性排序：範圍越具體越前面（Task > Goal > Workspace > 全域）→ 標籤出現在 Task 標題或契約中的較前面 → 最後更新時間由新到舊。決策放標題與內文；教訓摘要放標題、觸發條件與做法（根因與證據不放）。每一項以 `〔條目識別 v內容版本〕標題` 開頭。
5. 未讀訊息摘要（票 18 的格式，一個整體）；放進 Prompt 才把列出的訊息標為已交付。
6. 證據索引：這個 Task 與它直接依賴的 Task 已登記成果的最新版本（`〔成果識別 v版本〕名稱`、所屬 Task、sha256 前 12 碼、說明），只列索引，不放內容。

**預算**：`DEFAULT_BUDGET_TOKENS = 8000`（估計值：非 ASCII 字元每字 1、ASCII 每 4 字元 1）。必要項目一定放入；其餘依優先序逐項放入，放入後整個 Prompt 會超出預算的項目整項省略（不截斷），記為 `omitted`、原因 `budget_exceeded`，後面較小的項目仍可放入。

**receipt**（`ContextReceipt`）：`run_id`、`task_id`、`built_at`、`budget_tokens`、`used_tokens`（整個 Prompt 的估計值）、`prompt_sha256`，以及依優先序的 `items`（`kind`：`contract`／`handoff`／`checkpoint`／`decision`／`lesson`／`messages`／`evidence`；`ref_id`；`version`：契約版本、知識內容版本、成果版本、檢查點的世代，未讀摘要為 null；`title`；`tokens`；`status`：`loaded`／`omitted`；`omitted_reason`）。交接時的檢查點包含在交接說明中，記為 0 token。Prompt 與 receipt 由同一次組裝產生，之後都不再改變（Runtime 重啟後以派工 Outbox 中固定的 Prompt 啟動）。

| 端點 | 用途 |
| --- | --- |
| `GET /api/v1/workspaces/{id}/runs/{run_id}/context` | `{ receipt, prompt }`：receipt 與派工 Outbox 中實際交給 Worker 的 Prompt。Run 不存在 404 `run_not_found`；本票之前建立的 Run 沒有 receipt，404 `context_receipt_not_found` |

資料表（migration 35 `context_receipt`）：`context_receipt`（每個 Run 一列，追加式，觸發器禁止更新與刪除；與 Run 在同一個交易中、在 `run` 列之前寫入，所以 `run_id` 的外鍵是 `DEFERRABLE INITIALLY DEFERRED`）。桌面殼的 Tauri 指令：`run_context`。

測試：`crates/baton-harness/tests/context_builder.rs`（情境測試）、`crates/baton-domain/src/context/tests.rs`。

### Worktree、輸出目錄與整合佇列

Task 有**種類**（`kind`：`development`／`research`／`document`，建立時決定、預設 `research`）。規則在 `baton_domain::workdir` 與 `baton_domain::integration`，持久化在 `baton-store` 的 `workdirs.rs`、`integration.rs`（migration 21：`task.kind`、`task_workdir`、`integration_entry`），準備目錄在 `baton-api` 的 `workdirs.rs`，整合由 Supervisor 執行。

**工作目錄**：開發 Task 在 `<Workspace>\.baton\worktrees\<Task>`（從 Workspace 目前簽出的分支建立平台分支 `baton/<Task>`，該分支即整合目標），研究與文件 Task 在 `<Workspace>\.baton\outputs\<Task>`。放在 Workspace 目錄內，是因為 Workspace 授權的目標就是 Workspace 目錄；`.baton\.gitignore`（內容 `*`）讓主 checkout 不出現未追蹤的檔案。

- **派工之前準備**：使用者要求派工（`POST …/dispatch`）與 Supervisor 排程前，對排隊中的 `ready` Task 在交易外建立目錄（`WorkspaceFs::inspect_git`、`create_worktree`、`ensure_directory`），成功或失敗都登記到 `task_workdir`（事件 `task_workdir_prepared`／`task_workdir_failed`）。派工（`run::check_dispatch`）只認已登記且可用的目錄；不可用時不派工，排隊原因的類別是 `workdir`（`workdir_not_prepared`、`workdir_unavailable`，訊息就是登記的失敗原因，例如「這個 Workspace 不是 Git repo」）。失敗後排程每 60 秒重試一次；使用者再次要求派工時立即重試。建立是冪等的：建好之後、登記之前崩潰，下次準備會沿用已存在的 worktree。
- **一個 Task 一個目錄**：同一個 Task 的後續 Run 回到同一個目錄（原生 Session 以工作目錄區分專案）；不同 Task 的目錄一定不同（可用的路徑有唯一索引）。工作面板 `GET tasks` 回傳 `workdirs`（每個附 `run_ids`：派工時固定在啟動資訊中、工作目錄相同的 Run）。
- 確定性檢查的命令與檔案檢查在 Run 的工作目錄中執行（研究 Task 是輸出目錄，不是 Workspace 目錄）。

**整合佇列**（開發 Task）：成果接受交易（確定性檢查全部通過或手動接受）在同一個交易中把平台分支排入 `integration_entry`，Task 停在 `verifying`（`VerificationView.basis` 變成 `null`，不能再接受一次：409 `acceptance_already_queued`）。Supervisor 在驗證之後依序處理，一次一個（處理中的項目有唯一索引保證最多一個）：

1. `merging`：先持久化，再在整合目錄 `<Workspace>\.baton\integration` 以 detached HEAD 簽出目標分支並 `--no-ff` 合併（`WorkspaceFs::merge_for_integration`）；目標分支本身不動。衝突 → `failed`。
2. `verifying`：以 Task 契約的同一份確定性檢查，在整合目錄中執行整合驗證。任一未通過或不完整 → `failed`，證據保存在項目的 `checks`。
3. `advancing`：把目標分支從合併當時的 commit 以 fast-forward 前移到合併結果（`advance_branch`，冪等）；整合期間目標分支被更新 → `failed`。
4. `integrated`：Task 進入 `completed`。

`failed` 時 Task 回到 `ready`（可修正、可重新派工；重新派工的 Run 回到同一個 worktree），目標分支不受影響。Runtime 在 `merging`／`verifying` 時崩潰，重啟後從合併重做；在 `advancing` 時崩潰，以同一個合併結果重試前移。契約沒有宣告檢查（手動接受）時，乾淨的合併就是整合的證據。端點：`GET /api/v1/integration-queue`（全域佇列：`active` 與依排入順序的 `entries`）；工作面板另回傳 `integrations`，`GET …/verification` 另回傳最新的 `integration`。

**正式版 Git 操作**（`baton_ports::git_*`，以 `git -C <目錄>` 執行、不經過 shell）：整合目錄是主 checkout 或簽出分支的 worktree 時拒絕重設；前移時，若有工作目錄簽出目標分支就在那裡 `merge --ff-only`（有未提交的變更擋住時回報「無法前移」），否則以 `update-ref` 比對舊值後前移。只在 `user.name`／`user.email` 沒有設定時才以 `Baton <baton@localhost>` 作為合併 commit 的作者。測試：`crates/baton-ports/src/git.rs`（真實 git）、`crates/baton-harness/tests/worktrees.rs`（情境）。

**Worktree 自動清理**（票 21，ADR-0002）：規則在 `baton_domain::cleanup`，持久化在 `baton-store` 的 `cleanup.rs`（migration 24 `worktree_cleanup`，與 `task_workdir` 一對一），由 Supervisor 在整合之後、同一個背景工作中執行。

- **觸發**：平台登記且可用的 worktree、Task 是 `completed`、這個 Task 最新的整合項目是 `integrated`、它的 Run 全部結束時，登記為 `pending`（事件 `worktree_cleanup_requested`）。暫停、交接、失敗、取消、完全退出都不會讓 Task 完成，所以不觸發；研究與文件 Task 的輸出目錄不清理。完全退出中或等待「繼續工作」的 Workspace 不登記也不執行清理（不在這時執行 git），「繼續工作」後接著處理。
- **每一次嘗試**：先持久化 `removing`（`worktree_cleanup_started`），再重新核對安全條件（`WorkspaceFs::inspect_worktree`）：目錄必須仍是 repo 登記、簽出平台分支 `baton/<Task>` 的 worktree（主 checkout 或其他目錄一律視為歸屬不明），而且沒有未提交的變更或未追蹤的檔案（被 `.gitignore` 忽略的建置輸出不算）；目錄已不存在也算安全（只剩殘留要收拾）。通過才 `WorkspaceFs::remove_worktree`：把目錄改名到 `<Workspace>\.baton\trash\<Task>`（檔案被占用時在這一步失敗，什麼都沒變）、`git worktree prune` 清掉 Git 的登記、再刪除暫放目錄。不刪除分支、歷史或成果（成果在成果儲存，證據在控制資料庫）。
- **失敗**：安全條件不符或移除失敗時回到 `pending`，`failure` 是給使用者看的原因，`next_attempt_at` 依退避（1、2、4、8、10 分鐘）重試（`worktree_cleanup_retry_scheduled`）；第一次加上 5 次重試都失敗就是 `failed`（`worktree_cleanup_failed`），不再自動重試、worktree 保留。Task 一律維持 `completed`。
- **崩潰**：停在 `removing` 的登記在 Runtime 重建後立即接續（不另計嘗試次數），重新核對後完成；改名之後崩潰時目錄已不在原處，只會收拾平台自己的暫放目錄。
- 工作面板的 `workdirs[].cleanup` 是清理登記（`status`：`pending`／`removing`／`cleaned`／`failed`，`attempts`、`failure`、`next_attempt_at`、`finished_at`），卡片在工作目錄區塊顯示「待清理／清理中／已清理／清理失敗」，與 Task 的完成狀態分開。

測試：`crates/baton-domain/src/cleanup/tests.rs`、`crates/baton-ports/src/git.rs`（真實 git，含 Windows 檔案占用）、`crates/baton-harness/tests/worktree_cleanup.rs`（情境）。

**已知限制**：清理失敗（重試用完）後沒有手動重試的操作，worktree 保留給使用者自行處理；手動保留現場、保留期限與空間管理面板留待後續版本。權限提示的路徑判定仍以 Workspace 目錄為基準（相對路徑以 Workspace 目錄解讀，不是 Run 的工作目錄）。整合未通過後不會自動重新排入佇列。

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
| `POST /api/v1/workspaces/{id}/runs/{run_id}/settings-snapshot` | 為 Run 產生唯讀設定快照；JSON 本文 `{ "version"? }`（可為 `{}`，省略時用最新版本；已准入的 Run 用准入時固定的 `settings_version`，指定別的版本回 409 `settings_snapshot_pinned`，票 24）；第一次建立回 201，之後回同一個綁定 200 |

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

**准入與核對（票 14）**：快照由 `baton_api::RunSettings` 產生，API 端點與 Supervisor 共用同一個實例：**同一個 Run 的請求依序處理**（不會兩個請求同時寫同一個目錄）；**已綁定的快照每次都核對**：目錄中的檔案必須與版本渲染出的檔案（含 `manifest.json`）完全相同，不一致回 409 `settings_snapshot_mismatch`。Supervisor 啟動 Run 前（准入之後）自動取得快照：尚未綁定就以 Run 准入時固定的 `settings_version` 建立（票 24），已綁定就核對；核對不過 Run 以 `launch_failed：settings_snapshot_mismatch…` 結束，**不啟動程序**。准入時還沒有採用任何版本的 Run 不帶共同設定啟動（之後採用的版本也不會套用到它）。

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
| 事件 | 從 `stream.jsonl` 追蹤（Runtime 重建後重新接上同一個檔案）：每個非空白行先送 `Transcript`（位置 = 第幾個非空白行，從 1 起，重播時不變），再送該行的事件：`system/init` → `Started`（原生 Session）、`assistant` 文字 → `Message`、`rate_limit_event` → `RateLimited`（見「額度觀測」）、`result` → `UsageReported`，失敗時再 `Error`，最後 `Exited`；有新輸出就是 `Heartbeat` |
| 原生 Session | 第一次回報時以 `BindNativeSession` 綁定到 Run（`run.native_session_id`，事件 `run_native_session_bound`）；之後相同不變，不同則只記錄 `run_native_session_conflict`，**不改綁定** |
| 失敗原因 | `result` 的 `is_error` 為 true（即使 `subtype` 是 `success`）時，Run 以 CLI 回報的文字作為 `end_reason` |
| resume | `RunSpec.resume_native_session_id` 有值時加上 `--resume <id>`；`resume_capability` 查 `<CLAUDE_CONFIG_DIR>\projects\<工作目錄的非英數字元換成 -（超過 200 字元時回 Unknown）>\<id>.jsonl`：存在 → `Resumable`、不存在 → `NotResumable`、讀不到或資訊不足 → `Unknown`（實測與細節見 [adapter-contract.md](adapter-contract.md)） |

ProcessControl（`windows_process.rs`）依 `SpawnSpec` 的 `env_remove`、`stdin_file`、`stdout_file`、`stderr_file` 處理環境與標準輸入輸出；輸出檔以 share read/write/delete 開啟，Adapter 可在程序執行中讀取。

**模型（spec D-30）**：平台預設不指定模型。Workspace 可依角色覆寫：`PUT /api/v1/workspaces/{id}/model-overrides`，本文 `{ "controller"?, "worker"?, "reviewer"? }`（`null`、省略或空白 = 不覆寫；名稱只允許英數字與 `. _ - : [ ] @ /`，否則 400 `model_name_invalid`；封存的 Workspace 409 `workspace_already_archived`），回 200 與 Workspace（`model_overrides`）。派工時把 Worker 角色的模型固定進派工 Outbox（`LaunchPayload.model`），所以**只影響之後派出的 Run**。UI 在 Workspace 的「授權設定」畫面下方。

**Adapter 契約測試**：`crates/baton-harness/tests/adapter_contract.rs`，同一份契約跑在 fake、replay（真實 CLI 錄下的 stream-json）與 live（真實 CLI，手動）上；執行方式與結果記錄見 [adapter-contract.md](adapter-contract.md)。

### Session 持久化、沿用與替換

票 15 實作 issue §3.1～3.3：平台 Session（`agent_session_id`）、原生 Session（`native_session_id`）與程序身分（Run 上的 PID + 建立時間 + 指令雜湊）分開保存，規則在 `baton_domain::session`，Session 只隨 Run 命令在同一個交易中改變。

**何時決定**：Supervisor 寫入 launch intent（`BeginLaunch`）的同一個交易中決定沿用或建立 Session，所以原生識別取得前，平台 Session、launch intent 與 command_id 已經一起持久化；Runtime 在 spawn 前後任何一點當掉，都以 command_id 找回原程序，不會建立第二個 writer。寫入之前 Supervisor 先對**候選 Session**（同一個 Task 最近的 Session，帳號、工作目錄相同、已有原生識別、暫停中）呼叫 Adapter 的 `resume_capability`，結果與 launch intent 一起交給 Domain。

| 情況 | 結果 |
| --- | --- |
| Task 第一次啟動 | 新 Session（`allocating`，`driver_run_id` = 該 Run）；Adapter 回報原生識別後 `active` |
| 同帳號、同工作目錄，查驗為 `Resumable` | 新 Run 沿用同一個 Session（`session_resumed`），Run 的 `resume_native_session_id` 固定為原生識別，啟動參數帶 `--resume` |
| 查驗為 `NotResumable` | 前任標為 `unavailable`（`native_session_unavailable`），建立替代 Session |
| 查驗為 `Unknown`（或查驗失敗） | 不宣稱可續用：前任標為 `closed`（`resume_capability_unknown`），建立替代 Session |
| 換帳號／換工作目錄 | 不查驗；前任 `closed`（`account_changed`／`working_directory_changed`），建立替代 Session |
| 前一個 Run 沒取得原生識別就結束 | 該 Session 在 Run 結束時已是 `unavailable`（`native_session_never_bound`），建立替代 Session 並沿用這個原因 |

替代 Session 保存 `predecessor_session_id` 與 `replacement_reason`，前任的識別與原生識別不被覆蓋；替代 Session 一律有 `history_gap`（原生對話沒有延續，只能從平台保存的狀態重建），不宣稱已完整恢復。驅動 Session 的 Run 結束（任何結果）時：有原生識別的 Session 變成 `suspended`，從未取得的變成 `unavailable`。同一個 Task 再次派工時，Scheduler 優先選上一個 Run 的帳號（`scheduler::place_preferring`），才有機會原生接續。

**同一原生 Session 只有一個執行者**：平台 Session 的 `driver_run_id` 記錄目前驅動它的 Run，有執行者時不能被沿用（`session_busy`）；資料庫另有唯一約束（同一 Session 最多一個 `starting`／`running` 的 Run、同一個 `執行工具 + 帳號` 內原生識別只綁一個 Session）。Run 回報的原生識別已由同帳號另一個平台 Session 綁定時不綁定、記 `session_native_conflict`；若那個 Session 正由別的 Run 驅動，後到的 Run 被要求取消（`run_cancel_requested` 的 `reason` = `native_session_in_use`），Supervisor 停止它的程序。

**對話紀錄**：Adapter 的 `Transcript { position, line }` 依 Run 收集，`session::plan_transcript_import` 略過已保存的位置（重新接上串流時的重播）、跳號記為 `missing` 段落，每一行遮蔽秘密（JSON 行逐一遮蔽解碼後的字串；規則同共同設定的秘密辨識，另把 64 個十六進位字元的片段視為 Runtime 簽發的憑證）後，以 SHA-256 命名寫入成果儲存 `<資料目錄>\artifacts\transcripts\<sha256>.jsonl`，再於 `transcript_segment` 記錄位置區間、雜湊、大小、筆數與遮蔽數（同一個 Run 的區間不重疊）。

**端點**（桌面角色）：`GET /api/v1/workspaces/{id}/sessions`（`{ sessions: [...] }`，每個 Session 附 `run_ids` 與 `transcript`：stored／missing 段落數、每個 Run 已保存到的位置）、`GET /api/v1/workspaces/{id}/sessions/{session_id}`（`{ session, events, segments }`；`events` 來自 `session_events` 檢視；不存在 404 `session_not_found`）。Run 的回應多了 `agent_session_id` 與 `resume_native_session_id`。事件：`session_created`、`session_native_bound`、`session_resumed`、`session_suspended`、`session_unavailable`、`session_closed`、`session_native_conflict`。情境測試在 `crates/baton-harness/tests/sessions.rs`。

### Controller 長期對話與事件驅動

票 16 實作 spec D-06、D-31 與 ADR-0009（A1＋B1）：使用者在 Workspace 的「對話」分頁與 Controller 對話。規則在 `baton_domain::conversation`，持久化在 `baton-store` 的 `conversations.rs`（migration 30 `conversation_and_controller_run`），端點在 `baton-api` 的 `conversations.rs`，喚起與執行在 Supervisor 的 Controller 步驟。

**資料**：每個 Workspace 一個 `conversation`（識別由 Workspace 導出，第一則使用者訊息到達時建立）；`conversation_turn` 依 `seq` 記錄使用者訊息（`user_message`）、工作事件（`work_event`）與 Controller 回覆（`controller_reply`），輸入帶處理狀態 `pending`（UI 顯示「已收到／待處理」）→ `processing` → `processed`，並記錄處理它的 `controller_run_id` 與交付次數 `attempts`；`controller_run` 是 Controller 的一輪（一次 `claude -p`），只綁 conversation 與 Workspace、不綁 Task，所以不佔 execution lease、不影響任何 Task 狀態。

**事件驅動（A1）**：只有兩種輸入會喚醒 Controller：使用者訊息，以及 Worker 的成果或阻礙（Run 的 `run_succeeded`、`run_failed`、`run_blocked_reported`，在產生該事件的同一個交易中成為工作事件；同一個事件來源只會成為一個輸入）。Controller 自己提出的裁定請求被回答或過期時，也在同一個交易中成為工作事件（票 17）。Workspace 還沒有 conversation 時（使用者沒有找過 Controller）不建立工作事件。沒有待處理輸入就不喚醒，沒有任何定時巡查。

**喚起**：第一則待處理輸入到達後等 5 秒（合併視窗），期間到達的輸入併入同一輪。同一個 conversation 任何時刻最多一輪未結束（B1；資料庫的唯一索引 `controller_run_one_active` 也保證）。忙碌時新的輸入只保存，不中斷進行中的一輪；本輪結束後，下一輪先處理**全部**待處理的使用者訊息（依到達順序），沒有使用者訊息時才處理工作事件。帳號：只用核對無誤、額度門檻為 `open`（票 28 的遲滯：`paused`／`draining` 不接新的一輪）且用量未達停止派工門檻的帳號，可派時留在上一輪的帳號（才能原生接續）；Controller 不佔 Worker 的並行名額。完全退出的停止意圖生效或 Workspace 暫停時不喚起。

**喚起與結果保存兩個邊界**（與 Worker 的 launch intent 相同的做法）：

| 邊界 | 同一個交易中持久化 | 之後中斷時 |
| --- | --- | --- |
| 喚起 | Controller Run（`starting`，含 command_id 與 Prompt）、輸入標為 `processing`、沿用或建立 Session | 下一次 tick 以 command_id 找回原程序就補寫回執、繼續；找不到就以 `interrupted`（`launch_outcome_unknown`）結束，輸入退回 `pending` |
| 結果保存 | Controller Run 結束、輸入標為 `processed`、回覆（`Message` 文字依序以空行串接）成為一則 turn、Session 暫停 | 保存失敗時結果留在記憶體、下一次 tick 重試 |

**進行中的一輪的存活核對**：真實 Adapter 讀到 `Exited` 之前不會結束事件串流，程序沒寫出結果就死掉（崩潰、被終止、整機重開、Runtime 一起當掉）時串流只會停住。所以 Supervisor 在串流結束、或超過 execution lease（60 秒）沒有收到任何事件時，向 ProcessControl 查證程序（Runtime 重建後先以 command_id 找回）。存活或無法確定時不下結論，再等 60 秒才查證下一次；確認不在時，下一次 tick 再把串流讀乾淨一次（Adapter 可能還沒讀到最後寫出的結果），有結束事件就照常保存，仍沒有才以 `interrupted` 結束、輸入退回 `pending`。崩潰恢復（票 31）不停止 Controller 的輪次，由這個核對找回；恢復門檻擋住派工時也不喚起 Controller。

退回 `pending` 的輸入（中斷或程序以失敗結束）在上一輪結束後退避重新交付（1、2、4… 分鐘，上限 10 分鐘），Prompt 註明「可能已部分處理，請先核對目前狀態」。完全退出時 Controller 的輪次與 Worker 一起在寬限期內停止，確認停止的同樣以 `interrupted` 結束、輸入退回 `pending`，重新開啟後再喚起。

**Session 與 Prompt**：Controller Session 是 `agent_session` 中 `role = controller`、`conversation_id` 有值、`task_id` 為空的 Session，沿用與替換規則與票 15 相同（`session::plan_launch_for`）。沿用原生 Session 時 Prompt 只有本輪的輸入（`--resume`）；新的或替代的 Session 另附角色說明與最近 40 則已處理的對話，明示「只作為背景，不要重新執行」，所以換 Session 後 conversation 不變、未處理的指示只交付一次。Controller 在 Workspace 目錄執行，模型取 Workspace 的 Controller 覆寫（spec D-30），不套用共同設定快照。Controller 的 Agent 憑證與工具由票 17 提供（取代本票的佔位值 `controller-credential-not-issued`），見〈Controller 拆工與派工工具（票 17）〉。

**端點**（桌面角色）：`GET /api/v1/workspaces/{id}/conversation`（`{ conversation_id, busy, active_run, next_wake_at, turns, runs }`；Workspace 不存在 404 `workspace_not_found`）、`POST /api/v1/workspaces/{id}/conversation/messages`（`{ "text" }` → 201 `{ turn, busy }`；空白 400 `message_empty`、超過 20000 字 400 `message_too_long`、已封存 409 `workspace_archived`）。送出訊息從不同步喚起 Controller。暫停與取消（Run／Task 的取消、Workspace 暫停、完全退出）不經過對話佇列，由 Runtime 直接處理（DR-20261022-01）。事件：`conversation_created`、`conversation_input_received`、`conversation_inputs_dispatched`、`conversation_inputs_processed`、`conversation_inputs_released`、`controller_run_launch_started`、`controller_run_started`、`controller_run_native_session_bound`、`controller_run_succeeded`、`controller_run_failed`、`controller_run_interrupted`。情境測試在 `crates/baton-harness/tests/conversation.rs`。

### Controller 拆工與派工工具（票 17）

票 17 實作 spec D-05、D-15 與 ADR-0010：Controller 拆工、補契約、加相依、派工與取消，全部透過 Controller 專屬的 Agent 工具**提出**，由 Runtime 驗證後才接受；Controller 不直接寫資料庫、不能執行指令，也不能自己啟動 Worker。端點在 `baton-api` 的 `controller.rs`，規則在 `baton_domain::task`（`DecisionHold`、准入缺項 `decision_pending`）與 `baton_domain::decision`（`DecisionOrigin`、`ControllerQuestion`、`ControllerGate`、`ConversationNotice`），持久化在 `baton-store`（`tasks.rs` 的 `propose_task`、`decisions.rs` 的 `controller_question`／`controller_gate`、`conversations.rs` 的 `enqueue_notice`／`controller_run_active`），工具定義在 `crates/baton-mcp-shim`。

**Controller 憑證**：Supervisor 喚起 Controller 的一輪時（`launch_controller`：Controller Run 已以 `starting` 持久化、`prepare` 之後、`spawn` 之前）呼叫 `CredentialIssuer::issue_controller`，簽發 Agent 角色、綁定 Workspace 與那一輪的憑證（`ControllerScope`，存在 `credential.controller_run_id`，每一輪最多一張）；簽發失敗就不啟動，這一輪以失敗結束、輸入退回待處理。交給程序的方式與 Worker 相同：只經環境變數 `BATON_AGENT_TOKEN`（與 `BATON_RUNTIME_URL`），資料庫只存雜湊。它沒有時間上限，有效與否取決於那一輪是否仍是 `starting`／`running`。取代票 16 的佔位值 `controller-credential-not-issued`。

**角色分流**（`guard` 接受帶 Worker 範圍或 Controller 範圍的 Agent 憑證，兩者都沒有仍是 401 `credential_invalid`；它在解析請求之前就依工具路徑分流（`baton_domain::agent::CONTROLLER_TOOLS`），處理函式內另有同樣的檢查；shim 對兩種角色列出同一份工具清單，Controller 工具的說明註明「Controller 專屬」，由 Runtime 依憑證拒絕）：

| 憑證 | 呼叫 | 結果 |
| --- | --- | --- |
| Controller | 6 個 Controller 工具 | 那一輪仍是 `starting`／`running` 才接受，結束後 401 `credential_revoked`；請求帶的 `workspace_id`／`run_id`（Controller 的一輪）與憑證不一致 403 `agent_scope_mismatch` |
| Controller | Worker 工具（`get_task_contract`、三個回報工具、`request_decision`、`controller_action`、`register_artifact`、知識、外部操作、訊息、`confirm_handoff`） | 403 `worker_only`：不能冒充 Worker 的 Run 回報或寫入 |
| Controller | `permission_prompt` | 一律 `{behavior: "deny", status: "controller_denied"}`，不建立裁定請求：Controller 不能直接執行指令、寫資料或自己啟動 Worker |
| Worker | 6 個 Controller 工具 | 403 `controller_only` |

**工具**（`POST /agent/v1/tools/{工具}`；`propose_task`、`update_task`、`add_dependency`、`cancel_task` 成功時回 `{task, goal_id, changed, admission: {admissible, missing}}`，`admission` 與工作面板的准入預覽相同；其他錯誤與對應的桌面端點相同）：

| 工具 | 請求 | 行為 |
| --- | --- | --- |
| `propose_task` | `title`、`goal_id`（既有 Goal）與 `goal_title`（建立新 Goal）二擇一、`kind?`（`development`／`research`（預設）／`document`）、`contract?` | 201；Task 一律 `pending`，契約可以先不完整。需要建立 Goal 時，Goal 與 Task 在同一個交易判定與寫入，任一個被拒絕就什麼都不寫。`goal_id`／`goal_title` 不是剛好一個 400 `goal_choice_invalid` |
| `update_task` | `task_id`、`contract`（整份取代）、`note?` | 200；內容相同 `changed: false`、不新增版本；ready 的 Task 改契約退回 pending。已派工過的 Task 改完成條件或驗證檢查先經決策權限表（`modify_done_criteria`，見下） |
| `add_dependency` | `task_id`、`depends_on` | 200；自我相依 400 `dependency_self`、循環 409 `dependency_cycle`，重複新增是冪等的 |
| `dispatch_task` | `task_id`、`priority?` | `pending`／`ready` 的 Task 先做完整准入檢查：缺少授權能力時經決策權限表（`exceed_authorization`，見下），其他缺項 409 `task_admission_denied`（附 `missing`）；pending 的 Task 通過後先轉為 ready。之後走與桌面派工端點相同的流程：201 `{status: "started", run}` 或 202 `{status: "queued", task_id, priority, reasons}` |
| `cancel_task` | `task_id` | 200；與桌面取消相同，進行中的 Run 在同一個交易中被要求取消 |
| `propose_decision_request` | `question`、`reason`、`options`（2–8 個 `{id, label, tradeoff?}`）、`recommendation?`、`evidence?`、`urgency?`、`deadline_minutes?`、`blocking`（1–20 個依賴這個答案的 Task，只能是 `pending`／`ready`） | 201 `{decision_request_id, status: "waiting_decision", existing: false, message}`；同一個問題（正規化後相同）仍待處理時 200、`existing: true`，新列出的 Task 併入既有請求的阻擋範圍。內容無效或 `blocking` 不符 400 `decision_invalid`，Task 不存在 404 `task_not_found` |

**准入相同**：Controller 建立與修改的 Task 走與工作面板相同的 `task::decide`、`task::admission_report`；`dispatch_task` 與桌面的 `POST tasks/{task_id}/dispatch` 共用 `runs::dispatch_now`（準備工作目錄、Scheduler 選帳號、派工交易），所以 Controller 派出的 Task 不會繞過任何准入、相依、Guard 或容量判定。

**依決策權限表推導事項**（AI-227 要求不信任 Agent 自報的 `matter`）：Controller 工具不帶事項，由 Runtime 依實際變更推導，再以 `ControllerGate` 判定（規則同 `controller_action`，但提出者是 Controller 的一輪、目標 Task 必填）。兩個事項內建都是「必須送裁定」，Workspace 不能放寬：

| 事項 | 何時推導 | 核准之後 |
| --- | --- | --- |
| `exceed_authorization` | `dispatch_task` 時准入缺項含 `capability_not_granted` | 核准用掉一次，但**不會擴大授權**：Workspace 授權仍未涵蓋時 409 `authorization_not_expanded`，要使用者到授權頁修改授權後再派工 |
| `modify_done_criteria` | `update_task` 對已有任何 Run 的 Task：原本已填寫的 `done_criteria` 被改掉（文字或引用不同），或 `verification_checks` 有任何不同；補上原本缺少或未知的完成條件不算 | 下一次對同一個 Task 的這類變更消耗它，照常寫入新版本 |

沒有可用的核准時變更不執行，建立 `kind: authority`、`origin: controller` 的裁定請求（同一個 Task 同一事項仍待處理時回既有請求），回 409 `requires_decision`（`{error, message, status: "requires_decision", matter, decision_request_id, existing}`）；使用者拒絕時消耗一次，回 409 `decision_denied`。核准的使用記在 `decision_approval_use`（`run_id` 是 Controller 的一輪）。

**漸進拆工**（ADR-0010「邊釐清、邊調查」）：`propose_decision_request` 建立 `kind: agent`、`origin: controller` 的請求，阻擋範圍是 `blocking`。Task 的狀態不變（不進入 `waiting_decision`），而是在回答前多一個准入缺項 `decision_pending`（`task.rs` 的 `DecisionHold`，由 `baton-store` 的 `tasks.rs` 只載入 Controller 待回答的提問；Controller 的 `exceed_authorization`／`modify_done_criteria` 請求不另外擋准入，分別由授權檢查與 `update_task` 判定），所以不能標為 ready、不能派工；不依賴答案的 Task 照常派出。回答後缺項消失，但 Task 不會自動排隊，由 Controller 依答案更新契約後再派工。過期規則（`stale_reason`）：Controller 的提問問的是使用者的決定，不會因為契約變更而過期，只有依賴它的 Task 全部結束（或不存在）時才過期；Controller 的權限表請求仍依目標 Task 的契約版本過期。Controller 與 Worker 提出的請求不能合併（400 `decision_merge_invalid`，可以改用連結）。裁定中心的請求詳情帶 `origin`。

**喚醒 Controller**：Controller 提出的請求（含被合併進來的）被回答或過期時，`decision::wake` 不寫喚醒 Outbox，而是產生 `ConversationNotice`，在**同一個交易中**成為該 Workspace conversation 的工作事件（`conversations.rs` 的 `enqueue_notice`，`source` = `decision_wake:<請求>:<revision>`，同一個結果只成為一個輸入；內容由 `notice_content` 產生：處理方式、選項、說明與依賴的 Task，或過期原因），由〈Controller 長期對話與事件驅動〉的事件迴圈喚起下一輪。

**資料**（migration 33 `controller_tools`）：`credential.controller_run_id`（參照 `controller_run`，唯一）；重建 `decision_request`，加 `origin`（`worker`／`controller`，既有資料為 `worker`；權限提示只能由 Worker 提出），`run_id` 不再是對 `run` 的外鍵，改由觸發器 `decision_request_origin` 依來源檢查（Worker → `run`、Controller → `controller_run`）；重建 `decision_approval_use`，`run_id` 同樣改由觸發器檢查（`run` 或 `controller_run`）。這個 migration 列在 `FOREIGN_KEYS_OFF_MIGRATIONS`：`migrate` 在交易外關閉 `foreign_keys`、`apply_migration` 套用後提交前以 `foreign_key_check` 核對沒有任何違反，再開回 `foreign_keys`。情境測試在 `crates/baton-harness/tests/controller_tools.rs`。

### Controller 跨帳號交接

票 30 讓 Controller 沿用 Worker 的交接協議（票 28、issue §5.2）換帳號。規則在 `baton_domain::conversation`（`handoff_stop_due`、`handoff_reason_for`、`TurnOutcome::HandedOff`、`wait_for_account`），持久化在 `baton-store` 的 `conversations.rs`（migration 34 `controller_handoff`），推進在 Supervisor 的 Controller 步驟。Controller 的一輪很短，所以沒有另外的交接紀錄表：安全邊界就是一輪的結束，交接記在承接的那一輪上。

| 協議步驟 | Controller 的對應 |
| --- | --- |
| 1. 要求（先選定承接帳號） | 上一輪的帳號（conversation 最近的 Session 的帳號）門檻為 `paused`／`draining` 或無法使用時，下一輪的帳號由排程另選。進行中的一輪所在帳號進入 `draining` 時，只有找得到承接帳號才繼續下一步，否則留在原帳號 |
| 2. 安全邊界 | 讓進行中的一輪自己結束：從帳號進入 `draining`（或這一輪開始，取較晚者）起最多等 `CHECKPOINT_WAIT_MS`（2 分鐘），之後才以 `stop_grace` 要求舊程序停止；先查證，已不在就不再停止 |
| 3. 轉移 | 舊程序**確認停止**後，下一次 tick 把串流讀乾淨仍沒有結果，這一輪才以 `interrupted`、`end_reason` = `handed_off：…`（事件 `controller_run_handed_off`）結束，輸入退回 `pending` 且**不退避**；同一次 tick 的喚起在承接帳號上以新的 Controller Session（`account_changed`）開始下一輪，Controller Run 記下 `handoff`（`from_run_id`、`from_profile_id`、`reason`），conversation 記事件 `controller_handoff_transferred`。停不下來就維持原狀、下一次再要求，B1 保證不會有第二輪 |
| 4. 確認 | 承接的那一輪保存了結果（`succeeded`），conversation 記事件 `controller_handoff_confirmed` |

**限流與當掉**：一輪沒有保存結果就結束（程序以失敗結束、程序已不在），而它收到過 CLI 的額度用盡訊號、或帳號門檻（依最新額度推進後）是 `draining` 時，一律以 `handed_off` 結束，不算 Controller 的失敗、不退避。所以突然限流、為交接而停止、以及停止途中 Runtime 當掉重建後由存活核對找回的一輪，都走同一條路。完全退出時停止的一輪仍是一般的中斷。

**換帳號的原因**（`handoff.reason`）：上一個帳號門檻 `draining` → `quota_draining`；`paused` 或用量 ≥75% → `quota_near_limit`；其他（未核對、已封存等）→ `account_unavailable`。換過帳號之後新的 Session 在承接帳號上，之後的輪次留在那裡原生接續，不會自動換回。

**重建**：新的（含替代的）Controller Session 的 Prompt 除了角色說明、紀錄缺口與最近 40 則已處理的對話，另附 Workspace 最近 20 筆已做成的決策（問題 → 選項名稱、備註）與尚未結束的 Task 各自最近一筆被接受的檢查點摘要（最多 20 筆），都標明只作為背景；由 Store 在喚起的同一個交易中載入。沿用原生 Session 時不附。未處理的指示依到達順序只列一次，被中斷過的註明「可能已部分處理」。

**沒有可用帳號**：輸入（使用者訊息、工作事件）照常保存為 `pending`，不喚起 Controller；Supervisor 以排程的等待原因寫入 conversation 的 `wait_code`／`wait_message`／`wait_since`（原因不變時不寫入，改變時保留開始等待的時間，事件 `conversation_controller_waiting`），下一輪開始時清除。Controller 不佔 Worker 的名額，所以 Worker、取消、裁定等其餘功能不受影響。

**端點**：`GET /api/v1/workspaces/{id}/conversation` 多了 `waiting`（`{ code, message, since }`；只有輸入已到期卻沒有喚起時才有，其餘為 `null`），`runs[]` 多了 `handoff`（`{ from_run_id, from_profile_id, reason }` 或 `null`）。「對話」分頁顯示等待原因，Controller 回覆旁標示交接來源、原因與「已確認」，被交接出去的一輪標示「已交接」。情境測試在 `crates/baton-harness/tests/controller_handoff.rs`。

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
- `harness.restart()`：模擬重啟。舊的 Runtime 被有秩序地停止（記下正常結束，原位址不再回應），資料目錄與全部 fake 都保留。
- `harness.crash_restart()`：模擬 Runtime 意外終止後再次啟動（不記下正常結束），新的 Runtime 判定崩潰並開始崩潰恢復。要在某個邊界當掉，先讓那一步永遠不回來（`FakeStop::Hang`、`FakeStop::HangAfterStop`（程序確實停止但呼叫不回來）、`hang_next_spawn_at`），等 Supervisor 卡住後丟棄進行中的 `supervise`，再 `crash_restart`；`process_control.alive()` 列出仍存活的程序，用來斷言沒有第二個 writer。
- 監護（票 32）：`harness.start_guardian()` 在測試程序內啟動監護（從監看目前的 Runtime 開始，健康查詢回報 `active`），`harness.guardian_step()` 讓它處理一次並回傳 `GuardianStep`（監護啟動的 Runtime 成為目前的 Runtime）；`harness.kill_runtime()` 是 Runtime 被殺掉而沒有人重啟它，`harness.stop_runtime_after_exit()` 是完全退出後 Runtime 自行結束，`harness.open_app()` 是使用者開 App（沒有 Runtime 才以一般方式啟動），`harness.kill_guardian()` 是監護被殺掉（健康查詢回報降級）。`fail_next_guardian_start_early()` 讓下一次監護啟動的 Runtime 在匯入監護日誌前就結束；`on_next_runtime_probe(action)` 在監護下一次查詢 Runtime 時先執行動作（監護寫下嘗試之後、啟動之前會再查詢一次，用來重現與完全退出或維護的競態）；`guardian_starts()` 是監護啟動 Runtime 的紀錄，`guardian_journal()` 是監護日誌的內容。
- 完全退出：`POST /api/v1/lifecycle/exit` 之後以 `harness.wait_for_exit()` 等停止流程結束（Harness 的 Runtime 在退出後不會自行結束服務，由測試決定何時 `restart`）；`Harness::builder().exit_timing(..)` 縮短 30 + 10 秒的上限；`process_control.script_stop(identity, FakeStop::Result(..)／Hang／Fail(..))` 指定某個程序停不停得下來，`stop_graces()` 讀取每次停止收到的寬限期；`harness.lifecycle_file_path()` 是生命週期檔的位置。
- `Harness::builder().without_full_text_search()`：以 `RuntimeOptions.full_text_search = false` 啟動，視同 FTS5 不可用，知識搜尋退回 LIKE（`search_mode = like_fallback`，日誌記錄降級）；之後 `harness.set_full_text_search(true)` 再 `harness.restart()`，即模擬 FTS5 恢復可用、啟動時重建索引。
- `harness.clock`、`process_control`、`agent_adapter`、`quota_observer`、`workspace_fs`、`platform_info`、`secret_protector`、`claude_env`：具體的 fake，可腳本化回應並讀取呼叫紀錄。`claude_env` 可設定 Claude Code／Git 是否存在與版本（`set_claude`、`set_git`、`fail_tool_probes`）、在某個設定目錄完成原生登入（`log_in`，會像真實的 Claude Code 一樣把登入資料寫進該目錄）、模擬環境變數覆蓋（`set_env_override`）、讓開啟登入終端機時順便登入（`script_terminal_login`），並記錄開啟過的終端機（`login_terminals`）。
- 示範測試：`crates/baton-harness/tests/health_demo.rs`；本機 API 防護情境：`crates/baton-harness/tests/api_auth.rs`；帳號 Profile 與首次啟動精靈情境：`crates/baton-harness/tests/profiles_and_first_run.rs`；Workspace 與控制資料庫情境：`crates/baton-harness/tests/workspaces.rs`；Task 與契約情境：`crates/baton-harness/tests/tasks.rs`；Supervisor、Run、程序身分與取消情境：`crates/baton-harness/tests/runs.rs`；裁定中心與權限提示情境：`crates/baton-harness/tests/decisions.rs`；完整裁定中心（版本校驗、六種處理方式、批次、合併連結、決策權限表）情境：`crates/baton-harness/tests/decision_center_full.rs`；單次批准（E1）與修改授權情境：`crates/baton-harness/tests/one_off_approval.rs`；授權撤銷與啟動准入順序情境：`crates/baton-harness/tests/authorization_revocation.rs`；Agent API、範圍憑證與 MCP shim 情境：`crates/baton-harness/tests/agent_api.rs`（以 `tokio::io::duplex` 當 stdio，在程序內跑真正的 shim 對真實 API 回報）；Artifact 儲存、manifest、中斷恢復與下載驗證情境：`crates/baton-harness/tests/artifacts.rs`；多帳號排程情境：`crates/baton-harness/tests/scheduling.rs`；額度觀測情境：`crates/baton-harness/tests/quota.rs`；檢查點與主動交接情境：`crates/baton-harness/tests/handoff.rs`；突然限流與等待資源情境：`crates/baton-harness/tests/sudden_limit.rs`；知識、教訓與全文檢索（含降級）情境：`crates/baton-harness/tests/knowledge.rs`；Guard 與重犯事件情境：`crates/baton-harness/tests/guards.rs`；Context Builder 與 context receipt 情境：`crates/baton-harness/tests/context_builder.rs`；Outbox 核對、外部操作與寫入故障情境：`crates/baton-harness/tests/outbox_and_external_ops.rs`；桌面生命週期與完全退出情境：`crates/baton-harness/tests/lifecycle.rs`，關窗與重開視窗連回同一個 Runtime：`crates/baton-client/tests/lifecycle_client.rs`；Worktree、輸出目錄與整合佇列情境：`crates/baton-harness/tests/worktrees.rs`；Worktree 自動清理情境：`crates/baton-harness/tests/worktree_cleanup.rs`；修正與重試預算情境：`crates/baton-harness/tests/retry_budgets.rs`；Runtime 意外崩潰後的自動恢復情境：`crates/baton-harness/tests/crash_recovery.rs`；監護程序、有限自動重啟與降級情境：`crates/baton-harness/tests/guardian.rs`，重開 App 只補監護：`crates/baton-client/tests/guardian_client.rs`；Controller 拆工與派工工具、漸進拆工情境：`crates/baton-harness/tests/controller_tools.rs`；事件時間線與觀測指標情境：`crates/baton-harness/tests/timeline_and_metrics.rs`。

後續的票撰寫情境測試時，一律透過這個 harness 操作真實 API；不要直接呼叫內部函式或斷言資料表欄位。

### CI

`.github/workflows/ci.yml` 在 `windows-latest` 上執行 `pnpm test` 與 `pnpm build`。

## 產品名稱

產品名稱集中在 `crates/baton-domain/src/product.rs`（`PRODUCT_NAME`、`DATA_DIR_NAME`）與安裝設定 `src-tauri/tauri.conf.json`（`productName`、視窗 `title`）。`crates/baton-domain/tests/product_name.rs` 會檢查兩處一致；Web UI 畫面上的名稱取自 Runtime 回報，不寫死。改名時只需修改這幾處（以及既有使用者的資料目錄）。

## 平台辨識與支援期限表

- PlatformInfo Port 只回報事實（OS build、功能版本、版本別、原生架構、程序架構）；支援狀態由 `baton-domain::platform::assess` 依事實與 Clock 判定，所以到期判斷可用 fake Clock 測試。
- 正式實作在 `crates/baton-runtime/src/windows_platform.rs`：Windows 11 以 build ≥ 22000 判定（登錄的 `ProductName` 在 Windows 11 上仍可能寫 Windows 10），原生架構取自 `IsWow64Process2`。
- 支援期限表（`SUPPORT_TABLE`）的來源與查閱日期寫在同一檔案；新增驗證通過的功能版本、或查到官方日期時，一併更新 `SUPPORT_TABLE_CHECKED_ON`。
