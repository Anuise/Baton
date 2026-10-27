# GLOSSARY

本檔收錄通用多 Agent 協作平台的領域詞彙，只定義「這個詞指什麼、和誰有關、不要用什麼詞稱呼它」。

- 不放執行進度（哪張票做完了、哪個階段開始了）；進度看 `.scratch/` 下的票與 `ai-decisions.md`。
- 不放技術決策（用什麼資料庫、什麼語言）；決策與理由看 [`docs/adr/`](docs/adr/)。
- 詞彙來源：[issue #3](https://github.com/Anuise/issue-orchestrator/issues/3)（v0.4 技術設計）與 `.scratch/platform-rebuild-v1/spec.md`。
- 產品暫名 **Baton**（AI 暫定，待使用者確認，見 spec 的 D-01）；本檔以「平台」泛稱整個系統。
- 每個詞條固定三欄：**定義**、**關係**、**避免使用**（列出容易混淆、本專案不採用的同義詞）。

---

## 一、工作環境與工作單位

### Workspace
- **定義**：一個工作環境，包含資料來源、成員範圍、資源政策與共用授權。對應使用者指定的一個目錄，該目錄可以不是 Git repo。
- **關係**：擁有多個 Goal、Task 與一份共用授權，同時最多一個 Controller CLI；授權由使用者確認範本後才生效。每個 Workspace 各自有「完全退出後自動續跑」與「跨登入自動續跑」設定。所有範圍查詢都以 Workspace 為界。
- **避免使用**：project、repo、folder、專案（「專案」只用於 Project Brief、Project Map 這類被研究的對象）。

### Goal
- **定義**：使用者要達成的目標，含限制、成功條件、對 Workspace 授權的引用與版本。可以附加更窄的限制，不能放寬 Workspace 授權。
- **關係**：屬於一個 Workspace；由 Controller 拆成多個 Task；內容改變會讓受影響的 Task 與成果被標示重驗。
- **避免使用**：epic、project、需求單、objective。

### Task
- **定義**：一個可獨立派工與驗收的工作單位，具備固定的六項最小契約：目標、輸入與依據、範圍與限制、預期成果、完成與驗證條件、阻礙處理。狀態由執行結果與驗證條件推進，程序退出或 Worker 宣稱成功都不會讓它完成。
- **關係**：屬於 Goal；Task 之間有相依邊（不得有循環）；一個 Task 可有多個 Run（因交接、重試或恢復）；產出 Artifact；可屬於某個 Workflow。契約不完整的 Task 不能派工。
- **避免使用**：issue、ticket、job、card、work item（`.scratch/` 下的「票」是拆這個專案本身用的工具，不是領域詞彙）。

### Run
- **定義**：一次 Agent 執行。綁定一個 Session、一個帳號、環境、程序識別、權限世代（generation）、檢查點與結束原因。Worker Run 必須綁 Task；Controller Run 不綁 Task，只綁 Workspace（一次 Controller CLI 的開啟）。
- **關係**：屬於 Task（Worker）或 Workspace（Controller）；交接會結束舊 Run 並建立 successor Run，兩者屬於同一個 Task；Run 結束只產生「Run 結束」事件，不等於 Task 完成。
- **避免使用**：job、attempt（Stage 的 step_attempt 另有專義）、execution、process（程序只是 Run 的一個屬性）、session。

### execution lease
- **定義**：Task 的寫入所有權。派工時由 Run 取得、Run 結束（並確認程序已停止）時釋放；有效期間 60 秒，每次 heartbeat 續約。
- **關係**：同一個 Task 任何時刻最多一個尚未釋放的 lease；過期只觸發對程序的核對，不證明程序死亡；Worker 的回報帶 lease 的世代（generation），過期世代的回報不改變狀態。
- **避免使用**：lock、mutex、ownership token、heartbeat（heartbeat 是續約的訊號，不是 lease 本身）。

### launch intent
- **定義**：Supervisor 在啟動 Run 的程序之前先持久化的啟動意圖，含 `command_id`。
- **關係**：先於程序存在；Runtime 在啟動與回執之間當掉時，以 `command_id` 找回原程序，不重複啟動；找不到時 Run 標為 interrupted，不自動重送。
- **避免使用**：spawn request、launch ticket。

### Session
- **定義**：Agent 的對話脈絡，分三種識別，不可混用：`conversation_id`（已退役的 conversation，只出現在歷史資料）、`agent_session_id`（平台產生的穩定 Session 主鍵）、`native_session_id`（執行工具回報的原生識別，取得前為空）。另有 `process_identity`（程序身分）用來查驗存活，不是 Session。
- **關係**：一個 Session 對應多個循序 Run；跨帳號交接會建立新的 Session 並保存 predecessor 關係。同一原生 Session 任何時刻只能有一個有效執行者。Session 存在不代表程序存活、對話完整或可 resume。
- **避免使用**：thread、chat、context（context 指載入的資訊，見 context receipt）、把 Session ID 當 Run ID 或 PID。

### conversation（已退役，票 08）
- **定義**：舊的使用者與 Controller 長期對話：使用者在「對話」頁打字，Runtime 合併使用者訊息與工作事件後，以每輪一次 `claude -p` 喚醒 Controller。已被 Controller CLI 取代（見 ADR-0014）。
- **關係**：歷史資料（`conversation`、`conversation_turn`，以及 `controller_run` 中 `kind = 'turn'` 的列）保留在資料庫，不遷移、不刪除、不再寫入；一個舊 conversation 曾對應多個 Controller Session。
- **避免使用**：用來指 Controller CLI 中的互動、「對話頁」（已移除）。

### Artifact
- **定義**：Task 產生的成果，含位置、版本、雜湊、產生者、Task 關聯與驗證紀錄。大型內容放檔案儲存，只登記位置與雜湊。同一個 Task 內同名者是同一個 Artifact，再次登記只會追加新版本。
- **關係**：由 Run 產生、屬於 Task；被 Decision Request、checkpoint、Evidence Bundle 引用；被引用的 Artifact 受保護，不被清理。Artifact 通過驗證條件後才被接受；登記只表示內容已完整保存並有 manifest，不等於被接受，也不改變 Task 狀態。
- **避免使用**：output、deliverable、file、attachment。

### Artifact 版本
- **定義**：Artifact 的某一次登記結果，不可變：版本號、描述、大小、SHA-256、儲存位置、產生者 Run 與世代、時間。每個版本是獨立的檔案，不用成果名稱當檔名。
- **關係**：屬於一個 Artifact；新版本只追加，不覆寫舊版本的檔案與 manifest；預覽與下載前會重新核對雜湊與大小，不符時拒絕提供。
- **避免使用**：revision（授權與契約的 revision 另有專義）、覆寫、更新成果。

### manifest（成果清單）
- **定義**：Runtime 在控制資料庫保存的 Artifact 與 Artifact 版本紀錄（追加式、不可更新或刪除），說明「哪些檔案是成果」。manifest 只在檔案完整落地之後才提交。
- **關係**：「已登記」＝有 manifest 引用＝內容完整；沒有 manifest 引用的檔案不是成果；與事件 `artifact_registered` 在同一個交易寫入。
- **避免使用**：index、metadata、registry（泛稱）。

### 未登記檔案（unregistered file）與暫存檔
- **定義**：成果儲存中沒有被任何 manifest 引用的檔案，分兩種：暫存檔（staging，`.partial`，寫入中斷、內容不完整）與已改名但 manifest 未提交的檔案（unregistered，內容完整但仍不是成果）。
- **關係**：由 Runtime 中途中斷（或登記被拒）造成；不被當成成果、不自動刪除，在成果列表中顯示，讓恢復時可辨識；與 Task 狀態無關。
- **避免使用**：orphan file、垃圾檔、temp file（暫存檔只指 `.partial`）。

### Message（訊息）
- **定義**：Worker 之間交換研究結果或協商介面的一則通知，含 `message_id`、`thread_id`、`correlation_id`、類型（`note`、`question`、`answer`、`result`）、寄件 Run 與收件 Task。每個收件 Task 各有一筆交付，狀態由 `pending`（已存入）→ `delivered`（出現在 Run 啟動的未讀摘要）→ `read`（用 `read_inbox` 取走全文）。
- **關係**：屬於 Workspace；收件匣跟著 Task，交接後的新 Run 接得到前一個 Run 沒讀完的訊息；同一個 `message_id` 對同一個收件人只交付一次；Worker 自己拉取，Runtime 不在執行中途插話。訊息不能改變 Task 的歸屬或範圍，想這麼做的嘗試會被忽略並轉成給 Controller 的事件。
- **避免使用**：notification、chat、broadcast、command（訊息不是指令，也不是 Controller 與使用者的對話；那在 Controller CLI 中進行）。

### Account Profile
- **定義**：一個 Claude Code 帳號及其獨立的原生登入與 Session 狀態，是額度管理的單位。設定目錄有兩種來源（`config_dir_origin`）：平台在資料目錄下建立的（`managed`），或使用者登記的既有目錄（`external`，例如平台外也在用的 `~/.claude-Work-n`）。
- **關係**：每個 Run 綁定一個 Account Profile；平台不刪除任何設定目錄，封存 Profile 也不刪，外部目錄尤其不修改；額度觀測、並行上限與 draining 都以 Account Profile 為單位；各 Profile 之間不共用登入資料；共同設定以版本化快照提供，不直接共用。
- **避免使用**：account（單說 account 容易與應用程式測試帳號混淆）、user、seat、token。

### Controller 帳號
- **定義**：使用者為某個 Workspace 開啟 Controller CLI 時選定的 Account Profile。
- **關係**：Controller CLI 開啟期間不被派去執行 Worker，結束後恢復；同一個 Profile 可以同時是多個 Workspace 的 Controller 帳號；換 Controller 帳號會開新的 Session，不接續原帳號的原生 Session。
- **避免使用**：主帳號、Work-0（那是 `.scratch` 流程的本機慣例，不是平台概念）、work（使用者口語中的 work 指的是 Account Profile）。

### 共同設定（Shared Settings）
- **定義**：所有 Account Profile 的 Agent 共用的行為基準，從使用者平常使用的 Claude Code 設定匯入，分偏好、指示、hooks、MCP 伺服器、plugins 與市集五類。不含任何登入資料；登入與 Session 狀態只留在各 Account Profile 自己的設定目錄。
- **關係**：只讀白名單來源（`settings.json`、`CLAUDE.md`、`.claude.json` 的 `mcpServers`），絕不改寫使用者的原始設定；經匯入候選預覽、使用者採用後才成為設定版本；各 Account Profile 不直接共用它，Run 只使用自己准入的設定快照。
- **避免使用**：global settings、user settings（那是使用者原本的設定檔，是匯入的來源，不是共同設定）、config、profile 設定、同步（本平台只單向匯入，不回寫）。

### 設定版本（Settings Version）
- **定義**：使用者採用匯入候選時建立的共同設定版本，編號從 1 連續遞增，含內容、內容雜湊、來源指紋、排除清單與採用時間。建立後不可變。
- **關係**：每次採用都建立新版本，即使內容與前一版相同；舊版本內容永不改變；Run 以設定快照固定使用某一版本，之後採用的新版本不影響已准入的 Run；交接與恢復產生的新 Run 重新檢查新版。
- **避免使用**：revision（授權的確認版本稱 revision，兩者獨立編號）、設定檔版本、覆寫、更新（版本只會新增，不會被更新）。

### 匯入候選（Import Candidate）
- **定義**：從來源檔讀出、尚未採用的共同設定預覽：候選內容、內容雜湊（`candidate_hash`）、來源指紋、與目前版本的差異，以及沒有被匯入的項目與原因（排除清單，不含值）。不寫入任何地方。
- **關係**：採用時必須帶預覽的 `candidate_hash`，來源在預覽後改變就拒絕，讓使用者採用的一定是他看過的內容；採用後成為設定版本。登入相關與看起來像秘密的內容在這一步被排除，秘密辨識是啟發式規則，不保證完全涵蓋。
- **避免使用**：draft、pending settings、匯入結果（泛稱，預覽之後才有「採用」的動作時不夠精確）。

### 設定快照（Run Settings Snapshot）
- **定義**：某個設定版本渲染成 Run 專屬的唯讀設定目錄（`<資料目錄>\run-settings\<run_id>\`：`settings.json`、`mcp.json`、`CLAUDE.md`、`manifest.json`），內容與版本雜湊一致，`manifest.json` 記錄每個檔案的 SHA-256。
- **關係**：屬於一個 Run，第一次產生時綁定版本且不再改變；Run 啟動時以設定參數傳給 Claude Code，避免熱重載改變已准入的 Run（此效果仍待實測）；在 Windows 上以檔案唯讀屬性保護，不是安全邊界。
- **避免使用**：checkpoint（Run 的進度保存，另有專義）、備份、副本、config dump。

---

## 二、角色

### Controller
- **定義**：長期存在的協調角色，本身也是 Agent。負責與使用者對話、釐清目標、拆 Task、派工、協調訊息、評估成果與提出 Decision Request。由使用者在 Controller CLI 中直接操作（AI 暫定，見 ADR-0014；先前「每次需要判斷時才自動啟動一輪」的做法已於票 08 移除）。
- **關係**：透過工具介面向 Runtime 提交動作，不直接寫資料；以一個 Controller 帳號執行；每個 Workspace 同時最多一個 Controller CLI。Controller 評估不取代必要的獨立審查。
- **避免使用**：orchestrator、manager、coordinator、主控（`.scratch` 流程中的「主控 session」是另一回事）。

### Controller CLI
- **定義**：使用者從桌面 App 開啟、在終端機視窗中操作的原生互動式 Claude Code，扮演某個 Workspace 的 Controller。開啟期間在平台上是一個互動式的 Controller Run；視窗中的程序結束，這個 Run 就結束。
- **關係**：屬於一個 Workspace，以一個 Controller 帳號執行；透過 MCP shim 使用 Controller 工具；Worker 的結果以背景等待的方式通知它，平台不替它轉述對話，也不自動喚醒它。
- **避免使用**：對話頁、chat、conversation（舊的對話頁已於票 08 移除，conversation 已退役）、終端機頁簽（它開在外部視窗，不在 App 裡）、主控 session。

### Worker
- **定義**：接受明確委託並執行的 Agent，負責實作、研究、文件、審查、測試等大量工作；回報進度、checkpoint、成果與阻礙。角色與帳號分開管理，帳號不固定綁定角色。
- **關係**：由 Controller 提議、Runtime 驗證與選帳號後啟動；一個 Worker Run 對應一個 Task；可與其他 Worker 交換資訊，但不能經私下訊息改變工作歸屬或範圍；不得自行擴權或改寫完成標準。
- **避免使用**：agent（太泛，Controller 也是 Agent）、subagent、子代理、executor、bot。

### Reviewer
- **定義**：對成果提出 finding 的 Worker 角色。高風險工作必須由「非作者」擔任，缺席時 Gate 保持關閉。
- **關係**：屬於 Worker 的一種角色；產出 Review Finding；其獨立性不因 Session 重建而重置。
- **避免使用**：checker、QA（QA 易與 Verification 混淆）。

### Runtime
- **定義**：唯一的控制平面，常駐背景。負責驗證動作、排程、啟停程序、帳號綁定、訊息交付、持久化、交接、恢復與機械監控；不需呼叫模型的規則都由它執行。
- **關係**：接收 Controller 與 Worker 的工具呼叫與使用者的管理操作；是控制資料的唯一寫入者；與 Guardian 分開；與 Runtime 內部管理 Agent 程序的 supervisor 是不同概念。
- **避免使用**：daemon、server、backend、engine、orchestrator。

### Agent 憑證
- **定義**：Runtime 在 Run 准入時為該 Run 簽發的範圍憑證，綁定一個 Workspace、一個 Run 與一個權限世代（generation），只能使用 Agent 的工具介面，不能操作使用者的管理功能。只交給該 Run 的程序，不寫進 Prompt、設定檔或日誌。
- **關係**：身分與範圍一律取自憑證，不採信請求自稱的 Workspace 或 Run；Worker Run 的憑證有效與否取決於該 Run 是否仍是 Task 的寫入所有者（見 execution lease）：Run 結束或被接替後，它不能再改變任何狀態，過期世代的回報只保留為稽核紀錄；Controller Run 的憑證（Controller 憑證）在該 Controller Run 結束，或 Runtime 重啟後重新綁定（`rebound_at`）時失效；由 MCP shim 代 Worker 或 Controller 出示給 Runtime；與桌面 App 的憑證分開，兩者不能互相取代。
- **避免使用**：token、API key、密鑰（泛稱）、session（Session 是對話脈絡，不是憑證）。

### MCP shim
- **定義**：Worker 與 Controller CLI 用來呼叫 Runtime 工具介面的轉接元件，把工具呼叫（讀取任務契約、回報進度、保存 checkpoint、回報受阻、Controller 工具）轉送給 Runtime。本身不保存狀態、不做判斷。同一個執行檔另有兩個子命令：`controller-host`（Controller 宿主：在終端機中啟動 `claude`，向 Runtime 回報它的開始與結束）與 `wait`（Controller 在背景執行，長輪詢等待事件，有事件就印出摘要並結束）。
- **關係**：隨 Worker Run 或 Controller CLI 啟動，帶著該 Run 的 Agent 憑證；Controller CLI 中以環境變數 `BATON_MCP_SHIM` 取得它的路徑；Runtime 才是驗證與寫入的一方；執行工具的整合（Adapter）負責把它設定給 Worker。
- **避免使用**：proxy、bridge、plugin、MCP server（泛稱，shim 只是其中一個提供平台工具的實作）。

### Guardian（監護程序）
- **定義**：獨立的輕量程序，在同一次 Windows 使用者登入期間監看 Runtime，只對已確認意外終止的 Runtime 做有限次自動重啟。不派工、不裁定、不寫控制資料。
- **關係**：與 Runtime 共用同一個恢復事件的三次額度；完全退出、更新、解除安裝時被抑制；自身故障時 Runtime 繼續工作並顯示保護降級。
- **避免使用**：watchdog（在本專案中只指 CLI 的限流等待設定）、supervisor（指 Runtime 內部管理 Agent 程序的模組）、monitor、守護程序。

### 桌面 App
- **定義**：使用者唯一的人工操作入口，內嵌 Web UI，並提供系統匣與「完全退出」。
- **關係**：連回既有 Runtime，不自行成為控制平面；只有經桌面 App 提交的裁定才算人工正式裁定；負責開啟與結束 Controller CLI，但不承載與 Controller 的對話。
- **避免使用**：client、frontend、瀏覽器入口（第一版不提供一般瀏覽器入口）。

---

## 三、裁定、決策與知識

### Decision Request
- **定義**：需要使用者決定的事項。含問題、Agent 無法自行決定的原因、選項、取捨、Controller 建議、證據、阻擋範圍、緊急程度、期限與 goal_revision。
- **關係**：由 Controller 或 Worker 提出（Agent 憑證只能提出與查詢）；在「裁定中心」集中處理；提交時以 expected_revision 校驗，過期者標為 superseded 並重新整理；只阻擋受影響的工作；逾時不等於同意；相似請求合併或互相連結。
- **避免使用**：approval request、question、ticket、人工確認請求。

### Decision
- **定義**：Decision Request 經人工正式裁定後產生的正式決策，保存權威來源、理由、適用範圍、有效期間與 supersedes（取代關係）。
- **關係**：回應一個 Decision Request；影響受影響 Task 的狀態；被 Context Builder 載入 Run；被 Decision Map 引用而不重複保存；Agent 的建議與人工的 Decision 分開保存。
- **避免使用**：ruling（動詞「裁定」可用，名詞統一用 Decision）、choice、approval。

### Permission Prompt（權限提示）
- **定義**：Worker 以非互動權限模式執行時，未預先允許的工具操作經 `permission_prompt` 工具送到 Runtime 的詢問。Runtime 依 Workspace 目前生效的授權判定：範圍內自動允許並記錄；範圍外（含無法歸類）建立一個 Decision Request 並回「等待裁定」。
- **關係**：由 Worker 在 Run 中提出；對照 Workspace Authorization；範圍外的結果是 `kind = permission` 的 Decision Request，只阻擋提出提示的 Task；使用者回答「允許這一次」只是單次批准（由同一 Task 的同一操作消耗一次），不擴大 Workspace Authorization。
- **避免使用**：permission request、approval（回答權限提示的結果仍是 Decision）。

### Decision Authority Table（決策權限表）
- **定義**：列出 Controller 各類事項的權限等級：可自主處理、必須送裁定、只能人工發起。每個事項有內建等級，使用者可以把任何事項改成任何等級（收窄或放寬都可以）；生效等級是使用者的設定，沒設定時是內建等級。
- **關係**：與 Workspace Authorization 不同：Workspace 授權管 Agent 能做哪些操作，Goal／Task／Stage 只能收窄；決策權限表管 Controller 的哪些決定要送裁定，由使用者自由調整。Controller 嘗試「必須送裁定」的事項時不執行，改成建立 Decision Request。
- **避免使用**：權限政策、ACL、授權表（易與 Workspace 授權混淆）。

### Lesson
- **定義**：從事件與證據得出的教訓，保存觸發條件、根因或待驗假說、有效解法、禁用或重試限制、適用版本與重犯紀錄。是種類為「教訓」的 Knowledge Entry，可信狀態同 Knowledge Entry；Agent（Controller）提交的一律是「候選」，須由使用者在「知識」分頁確認才升為「已驗證」；使用者自己建立的直接是已驗證。
- **關係**：可轉為一個或多個 Guard（可機械化者），或納入相關 Run 的 context（不可機械化者）；範圍分全域、Workspace、Goal、Task，Agent 只能提交 Workspace、Goal 或 Task 範圍，且範圍以它所屬的 Run 決定；只有有效的 Lesson 會被 Agent 搜尋到。錯誤的 Lesson（含被否決的候選）以撤銷處理，保留歷史、不刪除。
- **避免使用**：memory、note、tip、learning、經驗（泛稱可用，但詞條與資料統一用 Lesson）。

### Guard
- **定義**：由 Lesson 轉成的可執行防護，形式為啟動前檢查、測試、工具限制或重試限制，用程式攔截而不只是提醒。
- **關係**：關聯一個 Lesson；一律由使用者啟用；在 Run 准入（派工）時檢查範圍涵蓋的 Task；來源 Lesson 不再有效（撤銷、淘汰、被取代、過期）時自動失效；擋下一次就是一筆「已攔下」的重犯事件（Recurrence），沒被攔下的重犯由使用者回報，兩者都追溯到來源 Lesson，用來衡量 Guard 的成效。
- **避免使用**：rule（太泛）、policy（指授權政策）、lint、guardrail。

### Knowledge Entry
- **定義**：依全域、Workspace、Goal、Task 範圍保存的知識，附來源、證據、可信狀態、有效期間、版本、標籤與取代關係。種類分決策（Decision）、教訓（Lesson）與一般知識（事實、約定或背景）。可信狀態有五種：候選（`candidate`，Agent 提交、尚未經使用者確認）、已驗證（`verified`，使用者建立或確認）、已撤銷（`revoked`，內容是錯的）、已淘汰（`retired`，曾經正確但已不適用）、已被取代（`superseded`，被另一個條目取代）。「有效」指已驗證且現在在有效期間內；其餘狀態與過期者都不是有效的。
- **關係**：可被搜尋、查看、確認、修正、撤銷、淘汰與取代；修正只追加新版本，舊版本保留；條目不可刪除，撤銷、淘汰、被取代都只改變狀態並留下歷史；只有有效的條目會被當作知識提供給 Agent；Agent 推論不能自動取代人工 Decision（只有使用者能建立已驗證條目或指定取代關係）。
- **避免使用**：memo、wiki、memory；「啟用／停用」（用「有效」與可信狀態描述）；「刪除」（條目只撤銷或淘汰）。

### 全文檢索降級
- **定義**：全文檢索索引無法使用時，知識搜尋改用逐筆比對條目文字的狀態。搜尋結果相同，只是較慢。
- **關係**：Runtime 啟動時判定；搜尋回應以 `search_mode = like_fallback` 標示、日誌留下紀錄、「知識」分頁顯示提示。索引是可重建的衍生資料，每次啟動都從條目重建，所以恢復後降級期間寫入的條目一樣搜得到。
- **避免使用**：搜尋失敗、離線搜尋、fallback mode（泛稱）。

### Context Receipt
- **定義**：每個 Run 啟動時保存的紀錄，說明載入了哪些決策、教訓與其版本，以及因預算而省略了什麼。
- **關係**：由 Context Builder 產生、屬於 Run；用來事後追查 Worker 當時知道什麼。
- **避免使用**：prompt log、context dump。

---

## 四、額度、交接與恢復

### handoff（交接）
- **定義**：把一個 Task 的工作從舊 Run 轉給 successor Run，通常因帳號額度接近上限而觸發。依五個 phase 進行：準備交接、保存進度、停止舊寫入、轉移所有權、查驗接手。
- **關係**：結束舊 Run、建立 successor Run，Task 不變；以獨立的 handoff 紀錄保存兩者關係與 phase；前置條件是 draining；舊程序確認停止前，新 Worker 不能進入同一可寫目錄；不消耗修正預算。
- **避免使用**：failover、migration、切換帳號、transfer、重派（重派是另一個 Run 的派工，不保證延續）。

### checkpoint（檢查點）
- **定義**：Run 在安全邊界保存的進度版本，含目標、決策版本、成果、未完成步驟、失敗方法、未讀訊息與外部操作狀態。
- **關係**：由 Worker 或 Runtime 在交接前產生；successor Run 與恢復 Run 以最近的有效 checkpoint 加上實際成果為依據；突然限流時不依賴最後一輪摘要成功。
- **避免使用**：snapshot（設定快照另有專義）、save point、summary、摘要（摘要不能取代實際成果）。

### draining
- **定義**：Account Profile 的狀態：不再接受新派工，並對進行中的 Run 觸發交接。
- **關係**：由用量門檻或已觀測的限流警告觸發；是 handoff 的前置步驟；回到安全水位或觀測到重置後才恢復可派。
- **避免使用**：suspended（Session 的 suspended 是另一個狀態）、paused、disabled、cooldown（冷卻指同 Task 兩次換帳號的最短間隔）。

### Quota Sample（額度觀測）
- **定義**：對一個 Account Profile 在某時間點的用量觀測，含來源、observed_at、用量、reset_at（可未知）與可信度（已觀測／推估／未知）。
- **關係**：作為派工與 draining 判斷的依據；未知值不得填成零；本機看不到其他裝置的用量，標示觀測不完整。
- **避免使用**：usage（泛稱）、balance、剩餘量（只是其中一種欄位）。

### waiting_resource
- **定義**：Task 因所有合適帳號都受限而等待的狀態，進度已保存，依重置時間或退避輪詢重新評估。
- **關係**：與 waiting_decision（等人工裁定）、blocked（授權不足、相依未完成、狀態不明）分開呈現；不消耗修正預算。由突然限流的 handoff 進入，handoff 停在同名的 phase 並記下預計重新評估時間（重置後 1 分鐘，或 5 分鐘起、上限 60 分鐘的退避）；Stop Intent 生效時資源恢復也不派工。
- **避免使用**：queued（Run 的 queued 是另一回事）、rate-limited、stalled。

### 突然限流（sudden limit）
- **定義**：Run 執行中，Claude Code 回報帳號已達上限（exhausted）；Worker 已無法再呼叫模型，也來不及保存最後一輪的進度。
- **關係**：帳號進入 draining（原因 `limit_reached`）；觸發的 handoff 不受冷卻限制、不等 checkpoint，successor 以最近的有效 checkpoint 與實際成果接手；所有合適帳號都受限時 Task 進入 waiting_resource。
- **避免使用**：rate limit（泛稱，也指限流警告）、額度用完（不區分視窗與是否可恢復）、failure（突然限流不是失敗，不計入修正預算）。

### Recovery Event（恢復事件）
- **定義**：Runtime 意外終止後的一次完整恢復流程，最多三次自動嘗試（含第一次），第二與第三次前分別退避，連續穩定一段時間並持久化成功才結束。三次用盡後只能由使用者明確要求建立新輪次。
- **關係**：Guardian 的自動啟動與 Runtime 的核對、恢復、穩定觀察共用同一個事件的額度；與 Task 的修正預算、額度交接分開計數。
- **避免使用**：restart（只是其中一步）、retry（Task 的重試預算另有專義）、incident。

### Stop Intent（停止意圖）
- **定義**：使用者「完全退出」時持久化的意圖。存在期間，排程、交接、啟動重試與登入自啟動都不得建立新的 Worker。
- **關係**：優先於額度恢復、資源喚醒、自動續跑與恢復；跨 Windows 登入保留；只有使用者手動開啟 App 才進入「主動退出後重新開啟」流程。
- **避免使用**：shutdown flag、kill switch、取消（取消是針對 Task 或 Run）。

### Scheduler（排程）
- **定義**：Runtime 內決定「現在能不能派、派給哪個 Account Profile、不能派的話為什麼」的模組。依相依、優先序、授權、帳號狀態、額度門檻與並行容量判斷；判定與寫入在同一個交易中，兩個同時到達的要求不會搶到同一個名額。
- **關係**：派工要求（使用者或 Controller）交給它；有帳號與名額就建立 Run，否則進入派工佇列並回報等待原因；Supervisor 每次處理時讓佇列依序再試一次；不處理額度觀測、交接與 draining（那些改變的是「帳號能不能接新工作」，由 Quota 與 handoff 提供事實）。
- **避免使用**：dispatcher（Outbox Dispatcher 是另一個模組）、load balancer、queue manager。

### 派工佇列（dispatch queue）
- **定義**：已被要求派工、但目前沒有帳號或名額的 ready Task。只記錄「誰在排隊」與優先序；等待原因每次依目前狀態重新判定，不保存。
- **關係**：Task 離開 `ready`（派出、取消、契約被改而退回 pending）就離開佇列；失敗的 Run 讓 Task 回到 `ready` 但**不會**自動重新排隊（沒有重試預算前不自動重派）；與 waiting_resource（所有合適帳號都受限、進度已保存）是不同階段。
- **避免使用**：queued（Run 的 `queued` 是已建立、等 Supervisor 啟動）、backlog、pending（Task 的 `pending` 是契約尚未通過准入）。

### 等待原因（wait reason）
- **定義**：Task 還沒有開始的原因，分六類：名額已滿（capacity）、額度（quota）、授權（authorization）、相依（dependency）、狀態不明（state_unknown）、帳號（account：未登入、認證被覆蓋、身分不符、沒有任何帳號、帳號是開著的 Controller CLI 的 Controller 帳號 `controller_account`）。每個原因有穩定代碼與說明。
- **關係**：讓使用者知道該處理哪一類問題：名額已滿會自動恢復，額度要等重置，授權與相依要使用者或 Controller 處理，狀態不明與帳號要先核對帳號。
- **避免使用**：blocked（Task 的 `blocked` 是預算用盡後的狀態）、reason code（泛稱）。

### 並行容量與交接槽（capacity、handoff slot）
- **定義**：並行容量是每個帳號與全域同時進行的 Run 上限，有「保留交接容量」（預設：每帳號 2、全域 6，可用帳號數 ≥ 3 時保留 1 個交接槽）與「較高並行」（每帳號 3、全域 8、不保留）兩組預設，使用者可在設定頁切換。交接槽是保留給交接 successor 的全域名額，一般派工佔不到。
- **關係**：進行中 = 尚未結束的 Run（含已要求取消、程序還沒確認停止者）；切換預設不影響已在執行的 Run；交接槽由 handoff 使用。
- **避免使用**：slot（泛稱）、quota（額度是帳號用量，不是並行數）、concurrency limit。

### Admission（准入）
- **定義**：Run 啟動前最後一次授權與設定版本檢查，並持久化啟動許可。已准入的 Run 沿用當時的授權與設定版本。
- **關係**：建立 queued Run 或 Outbox 項目不等於已准入；撤銷授權不停止已准入的 Run；交接與恢復產生的新 Run 重新檢查新版。
- **避免使用**：approval（指人工批准）、launch（程序啟動發生在准入之後，且在交易之外）。

---

## 五、授權

### Workspace Authorization（Workspace 授權）
- **定義**：Workspace 共用的範圍授權，以「能力＋目標資源＋限制條件」表達。範圍內預先授權，例外送人工裁定。
- **關係**：Goal、Task、Stage 只能引用並收窄；具備某能力不是任意 Task 使用它的理由；管理員／全域安裝、遠端寫入、部署、對外發訊、付費、正式資料操作各需明確授權。
- **避免使用**：permission（泛稱，也用於 CLI 的權限提示）、ACL、角色權限。

### One-off Approval（單次批准）
- **定義**：綁定單一邏輯操作（含穩定識別、目標資源、重要參數、要求雜湊、成果版本）的例外批准，簡稱 E1。
- **關係**：不增加 Workspace 授權；換帳號或 Run 不自動失效，但執行時仍須核對有效性；操作完成後不能用於另一個操作；結果未知時先查證，不盲目重送。
- **避免使用**：allow once、臨時授權、白名單。

---

## 六、工作方法與流程

### Workflow
- **定義**：依選定 Playbook 版本推進的一組 Task 與 Stage 的實例。一個 Workflow 可含多個 Task。
- **關係**：不取代 Task 與 Run；記錄 Playbook 的選擇理由與版本；進行中的 Workflow 不因 Playbook 升級而改變。
- **避免使用**：pipeline、process、flow、工作流程（泛稱）。

### Stage
- **定義**：Workflow 中有穩定身分的階段，宣告進入條件、輸入、預期產出、允許動作、預算、離開檢查、失敗路徑與 checkpoint 政策。可有多次 step_attempt。
- **關係**：由 Gate 把關；依證據推進，不接受文字宣告「做完」；跳過需保存原因，且不得跳過使用者明定的裁定或安全門檻；交接不會讓 Stage 從頭重跑。
- **避免使用**：phase（handoff 的 phase 是交接的內部步驟）、step、milestone。

### Gate
- **定義**：Stage 離開條件的檢查點，須有必要證據與必要審查才開啟。需求、build 或 Verification Profile 改變時，相關 Gate 會失效並建立重驗工作。
- **關係**：屬於 Stage；Standards 與 Spec 兩軸 Review 任一不通過都保持關閉；缺少非作者 Reviewer 時保持關閉。
- **避免使用**：checkpoint（檢查點是 Run 的進度，不是 Gate）、approval、sign-off。

### Playbook
- **定義**：版本化的工作流程範本，依工作情境選用。第一版內建七個：需求模糊、功能開發、缺陷處理、架構或廣泛遷移、效能改善、調查與研究、探索測試。
- **關係**：由 Method Registry 保存、由 Playbook Router 依情境選擇；版本不可原地覆寫；研究與文件工作不被強制套用開發流程。
- **避免使用**：template、SOP、skill（本 repo 中 skill 指 Claude Code 技能）、runbook。

### Project Brief
- **定義**：Workspace 首次接入專案時由 Controller 建立並維護的專案概覽：產品目標、使用者、成功條件、排除範圍、系統現況、外部整合、風險與可執行限制。
- **關係**：與 Glossary、Project Map、Decision Map 並列為「專案理解」；先從 repo 與文件找答案，只把真正的產品取捨送入裁定。
- **避免使用**：README、PRD、spec（spec 是本專案的規格文件，不是領域物件）。

### Project Map
- **定義**：模組責任、介面、主要使用路徑、資料流、測試入口與相關決策索引。每項附來源位置、版本與查驗時間；未確認的關係標為假說。
- **關係**：協助 Controller 不必讀完整個專案；相關模組或規則變動後重新檢查。
- **避免使用**：architecture diagram、code map。

### Decision Map
- **定義**：大型目標的決策地圖：目的地、已解決問題索引、可處理問題、尚未清楚到能拆 Task 的區域與明確排除範圍。
- **關係**：引用正式 Decision，不另存答案；依賴人工回答的部分保持等待，可並行的研究先派出。
- **避免使用**：roadmap、plan（Decision Map 刻意不預先排定完整計畫）。

### Design Package
- **定義**：非平凡架構變更前產生的設計內容：領域模型、狀態轉換、責任歸屬、呼叫端用法、介面契約、失敗模式與測試觀察點。
- **關係**：實作偏離時保存 deviation 並更新同一份設計與版本；高影響設計比較至少兩個結構不同的草案。
- **避免使用**：design doc、RFC。

### Review Finding
- **定義**：Reviewer 提出的審查發現，分 Standards（規範）與 Spec（需求）兩軸保存，固定 base revision、candidate revision 與規格版本。由 Controller 以證據分類為「修正」、具理由駁回、或需裁定。
- **關係**：影響 Gate；駁回 finding 必須送裁定；修正後只有可證明不受影響的審查可沿用。
- **避免使用**：comment、nit、issue。

---

## 七、驗證

### Verification Profile
- **定義**：專案描述如何驗證真實介面的內容：介面、啟動命令、doctor、就緒條件、版本、端點、資料準備、帳號權限參照、隔離方式、操作入口、斷言、證據保存與 teardown。「就緒」必須代表可使用，不能只是程序存在。
- **關係**：由 Verification Adapter 執行；改變時使相關 Gate 失效；搭配 Feature Map 以使用者旅程對應驗收準則。
- **避免使用**：test config、e2e config、環境設定。

### Verification Step 與 verdict
- **定義**：驗證的一個步驟與其判定。判定分為 passed、product_failure、inconclusive、environment_blocked、auth_blocked、budget_exhausted、automation_unsupported，來源引擎的原始錯誤碼另存。
- **關係**：操作與判定分離；證據不足不算通過，環境故障或模型限流不算產品缺陷；單一案例未完成不得拼接為 passed。
- **避免使用**：pass/fail（二元判定會掩蓋其他五種結果）。

### Evidence Bundle
- **定義**：綁定 app/build、candidate revision、需求版本、Profile 版本、資料集、測試 Session、動作、斷言、判定與原始產物的證據集合。截圖、錄影、trace、報告都存為 Artifact。
- **關係**：支撐 Gate 與 Task 完成；憑證與個資依政策遮蔽；清理只移除本 Run 的測試狀態，保留證據。
- **避免使用**：test report、log dump、截圖集。

### 確定性檢查（deterministic check）
- **定義**：契約「完成與驗證條件」中可由 Runtime 機械判定的部分，隨契約版本保存：命令結束碼、工作目錄內檔案存在與雜湊、Task 成果存在與雜湊。
- **關係**：Run 成功結束、Task 進入 `verifying` 後由 Runtime 執行；全部通過才讓 Task 完成，任一未通過讓 Task 回到 `ready` 並保存證據。沒有宣告時只能由使用者手動接受。
- **避免使用**：自我檢查、Worker 驗證（檢查由 Runtime 執行，不採信 Worker 的說法）。

### 成果接受（acceptance）與驗證紀錄
- **定義**：讓 `verifying` 的 Task 進入 `completed` 的唯一交易：依確定性檢查結果或使用者以桌面憑證手動接受，並同時校驗受驗 Run 的世代、契約版本與成果 manifest 仍與觀察當時相同。每次結論保存為一筆不可變的驗證紀錄（方式、結果、證據、manifest、接受者指紋）並追加事件。
- **關係**：接受基礎（basis）是觀察當時的 Run、世代、契約版本與 manifest；過期 Run 的回報與依過期基礎的接受都被拒絕。同一個 Run 最多一筆紀錄，同一個 Task 最多一筆通過。
- **避免使用**：Worker 宣稱完成、程序正常退出（兩者都不等於完成）、approve（易與裁定混淆）。

### Finding 與 confirmed defect
- **定義**：探索測試的發現。先標為 suspected；排除 seed data、工具限制與預期設計後，建立對應原症狀的重現測試，並確認以產品行為斷言失敗，才升級為 confirmed defect；無法重現者保留為 unverified risk。
- **關係**：confirmed defect 修復後須納入正常回歸，維持需求到測試的追蹤。
- **避免使用**：bug（未經重現時不稱 bug 或 defect）、issue。

### Replay
- **定義**：只在後續有效斷言證明操作結果後才保存的動作錄製，重播時仍執行真實動作並重新檢查後置條件。
- **關係**：不是「曾經通過」的驗收結果，也不是跨帳號交接；`no_live_model` 模式必須由 Adapter 強制禁止所有即時模型呼叫。
- **避免使用**：cache hit、已通過（快取命中不是通過）。

---

## 八、生命週期

### 完全退出（full exit）
- **定義**：使用者從系統匣主動要求停止所有背景工作。先持久化 Stop Intent，再限時受控停止所有平台管理的程序；與「關閉視窗」不同。
- **關係**：不等於取消 Task，未完成的 Task、Workflow、Session 關聯、成果與裁定都保留；停止過程不依賴最後一次模型回應；無法確認停止的程序記為停止異常。
- **避免使用**：quit（泛稱）、kill、關閉視窗（關閉視窗後工作繼續）、取消。

### 自動續跑（auto-resume）
- **定義**：再次開啟後是否自動恢復工作的 Workspace 層級設定，預設關閉。分兩種且互相獨立：「完全退出後自動續跑」與「跨登入自動續跑」。另有目前 Windows 使用者層級的「登入時自動啟動」。
- **關係**：只決定後續啟動與恢復，不立即啟停目前工作，也不構成操作授權，不繞過待裁定、授權不足或未知副作用的阻擋。
- **避免使用**：auto-start（指登入自啟動）、resume（指 Session 的原生 resume）、全域續跑開關。

### Unknown Side Effect（結果未知的外部操作）
- **定義**：已送出但結果無法確認的外部操作（部署、發訊、付費等）。恢復時必須先查證，不能確認就送裁定。
- **關係**：以 command_id 與要求雜湊識別；不盲目重送；不因重新測試或重新派工而再送一次。
- **避免使用**：failed operation（失敗與未知不同）、pending。

### Worktree（工作副本）
- **定義**：程式開發 Task 專用的獨立 Git worktree。Task 通過驗證、成果整合、無人使用且成果已保存後自動移除；Task 完成與清理狀態分開保存（已清理／待清理／清理失敗）。
- **關係**：暫停、交接、失敗、取消與完全退出都不觸發清理；只移除平台建立並登記的目標；研究與文件 Task 改用獨立輸出目錄。位置是 `<Workspace>\.baton\worktrees\<Task>`，簽出平台分支 `baton/<Task>`；同一個 Task 的每個 Run 都在同一個 worktree 中執行。
- **避免使用**：checkout、workdir、sandbox（不構成安全隔離）。

### 備份（Backup）
- **定義**：控制資料庫的一致快照（SQLite Online Backup API）加上快照中登記的成果檔案與 manifest，存放在 `<資料目錄>\backups\`。觸發原因是每日、migration 前，或重要裁定／Stage 完成（同一小時內合併為一次）；保留 7 份每日與 4 份每週。
- **關係**：不含任何憑證與登入資料（Profile 原生設定目錄、桌面憑證檔不在其中，快照中的憑證表已清空）。只能還原到乾淨的目錄；還原時，備份當下尚未結束的 Run 一律結束，不自動重做外部操作。
- **避免使用**：snapshot（指 Run 設定快照或資料庫快照本身）、checkpoint（Run 的進度保存）、匯出。

### 還原演練（Restore Drill）
- **定義**：每週一次，把最新的備份還原到隔離目錄，核對資料庫完整性、外鍵關聯、成果雜湊，以及資料庫登記的成果版本與 manifest 一致；結果顯示在設定頁。
- **關係**：只證明備份可用，不改變正式資料目錄；任一項核對沒通過就是不通過。
- **避免使用**：restore test（泛稱）、驗證（另指 Verification Profile 的驗證）。

### 輸出目錄（output directory）
- **定義**：研究與文件 Task 專用的獨立目錄（`<Workspace>\.baton\outputs\<Task>`），Workspace 不必是 Git repo。
- **關係**：與 Worktree 合稱 Task 的「工作目錄」：派工前由平台建立並登記，不可用時不派工並顯示原因；Worker 的程序以它為工作目錄。
- **避免使用**：output（單獨使用時與成果混淆）。

### 整合佇列（integration queue）
- **定義**：全部 Workspace 共用的單一佇列。開發 Task 的成果通過驗證後，平台分支排入佇列，依序在整合目錄（`<Workspace>\.baton\integration`）合併並執行整合驗證，通過才把目標分支前移、Task 才完成。
- **關係**：任何時刻最多一個項目在處理中；合併衝突、整合驗證未通過或目標分支在整合期間被更新時，Task 不完成而回到 ready，目標分支不受影響。「成果已整合」是 Worktree 自動清理的前提之一。
- **避免使用**：merge queue（指 GitHub 的功能時）、自動合併。
