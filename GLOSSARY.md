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
- **定義**：一個工作環境，包含資料來源、長期對話、成員範圍、資源政策與共用授權。對應使用者指定的一個目錄，該目錄可以不是 Git repo。
- **關係**：擁有多個 Goal、Task、conversation 與一份共用授權；授權由使用者確認範本後才生效。每個 Workspace 各自有「完全退出後自動續跑」與「跨登入自動續跑」設定。所有範圍查詢都以 Workspace 為界。
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
- **定義**：一次 Agent 執行。綁定一個 Session、一個帳號、環境、程序識別、權限世代（generation）、檢查點與結束原因。Worker Run 必須綁 Task；Controller Run 可只綁 conversation 與 Workspace。
- **關係**：屬於 Task（Worker）或 conversation（Controller）；交接會結束舊 Run 並建立 successor Run，兩者屬於同一個 Task；Run 結束只產生「Run 結束」事件，不等於 Task 完成。
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
- **定義**：Agent 的對話脈絡，分三種識別，不可混用：`conversation_id`（使用者與 Controller 的長期對話）、`agent_session_id`（平台產生的穩定 Session 主鍵）、`native_session_id`（執行工具回報的原生識別，取得前為空）。另有 `process_identity`（程序身分）用來查驗存活，不是 Session。
- **關係**：一個 conversation 對應多個 Controller Session；一個 Session 對應多個循序 Run；跨帳號交接會建立新的 Session 並保存 predecessor 關係。同一原生 Session 任何時刻只能有一個有效執行者。Session 存在不代表程序存活、對話完整或可 resume。
- **避免使用**：thread、chat、context（context 指載入的資訊，見 context receipt）、把 Session ID 當 Run ID 或 PID。

### Artifact
- **定義**：Task 產生的成果，含位置、版本、雜湊、產生者、Task 關聯與驗證紀錄。大型內容放檔案儲存，只登記位置與雜湊。
- **關係**：由 Run 產生、屬於 Task；被 Decision Request、checkpoint、Evidence Bundle 引用；被引用的 Artifact 受保護，不被清理。Artifact 通過驗證條件後才被接受。
- **避免使用**：output、deliverable、file、attachment。

### Account Profile
- **定義**：一個 Claude Code 帳號及其獨立的原生登入與 Session 狀態，是額度管理的單位。
- **關係**：每個 Run 綁定一個 Account Profile；額度觀測、並行上限與 draining 都以 Account Profile 為單位；各 Profile 之間不共用登入資料；共同設定以版本化快照提供，不直接共用。
- **避免使用**：account（單說 account 容易與應用程式測試帳號混淆）、user、seat、token。

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
- **定義**：長期存在的協調角色，本身也是 Agent。負責與使用者對話、釐清目標、拆 Task、派工、協調訊息、評估成果與提出 Decision Request。不是常駐程序，每次需要判斷時才啟動一輪。
- **關係**：透過工具介面向 Runtime 提交動作，不直接寫資料；同一 conversation 同時最多一輪進行中，忙碌時新訊息先保存；可像 Worker 一樣因額度而交接。Controller 評估不取代必要的獨立審查。
- **避免使用**：orchestrator、manager、coordinator、主控（`.scratch` 流程中的「主控 session」是另一回事）。

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

### Guardian（監護程序）
- **定義**：獨立的輕量程序，在同一次 Windows 使用者登入期間監看 Runtime，只對已確認意外終止的 Runtime 做有限次自動重啟。不派工、不裁定、不寫控制資料。
- **關係**：與 Runtime 共用同一個恢復事件的三次額度；完全退出、更新、解除安裝時被抑制；自身故障時 Runtime 繼續工作並顯示保護降級。
- **避免使用**：watchdog（在本專案中只指 CLI 的限流等待設定）、supervisor（指 Runtime 內部管理 Agent 程序的模組）、monitor、守護程序。

### 桌面 App
- **定義**：使用者唯一的人工操作入口，內嵌 Web UI，並提供系統匣與「完全退出」。
- **關係**：連回既有 Runtime，不自行成為控制平面；只有經桌面 App 提交的裁定才算人工正式裁定。
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

### Lesson
- **定義**：從事件與證據得出的教訓，保存觸發條件、根因或待驗假說、有效解法、禁用或重試限制、適用版本與重犯紀錄。狀態為「候選」或「已驗證」；候選須由使用者確認才升為已驗證。
- **關係**：可轉為一個或多個 Guard（可機械化者），或納入相關 Run 的 context（不可機械化者）；範圍分全域、Workspace、Goal、Task；錯誤的 Lesson 可撤銷並保留歷史。
- **避免使用**：memory、note、tip、learning、經驗（泛稱可用，但詞條與資料統一用 Lesson）。

### Guard
- **定義**：由 Lesson 轉成的可執行防護，形式為啟動前檢查、測試、工具限制或重試限制，用程式攔截而不只是提醒。
- **關係**：關聯一個 Lesson；一律由使用者啟用；在 Run 啟動前或操作時被檢查。
- **避免使用**：rule（太泛）、policy（指授權政策）、lint、guardrail。

### Knowledge Entry
- **定義**：依全域、Workspace、Goal、Task 範圍保存的知識，附來源、證據、可信狀態、有效時間、版本、標籤與取代關係。Decision 與 Lesson 都是 Knowledge Entry 的特定形態。
- **關係**：可被搜尋、查看、修正、撤銷與淘汰；Agent 推論不能自動取代人工 Decision。
- **避免使用**：memo、wiki、memory。

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
- **關係**：與 waiting_decision（等人工裁定）、blocked（授權不足、相依未完成、狀態不明）分開呈現；不消耗修正預算。
- **避免使用**：queued（Run 的 queued 是另一回事）、rate-limited、stalled。

### Recovery Event（恢復事件）
- **定義**：Runtime 意外終止後的一次完整恢復流程，最多三次自動嘗試（含第一次），第二與第三次前分別退避，連續穩定一段時間並持久化成功才結束。三次用盡後只能由使用者明確要求建立新輪次。
- **關係**：Guardian 的自動啟動與 Runtime 的核對、恢復、穩定觀察共用同一個事件的額度；與 Task 的修正預算、額度交接分開計數。
- **避免使用**：restart（只是其中一步）、retry（Task 的重試預算另有專義）、incident。

### Stop Intent（停止意圖）
- **定義**：使用者「完全退出」時持久化的意圖。存在期間，排程、交接、啟動重試與登入自啟動都不得建立新的 Worker。
- **關係**：優先於額度恢復、資源喚醒、自動續跑與恢復；跨 Windows 登入保留；只有使用者手動開啟 App 才進入「主動退出後重新開啟」流程。
- **避免使用**：shutdown flag、kill switch、取消（取消是針對 Task 或 Run）。

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
- **關係**：暫停、交接、失敗、取消與完全退出都不觸發清理；只移除平台建立並登記的目標；研究與文件 Task 改用獨立輸出目錄。
- **避免使用**：checkout、workdir、sandbox（不構成安全隔離）。
