# Spec：桌面頁簽化與原生 CLI Controller

Status: ready-for-agent

- 來源：[grilling.md](grilling.md)（Q1～Q17 的 AI 回答即本 spec 的決策）
- 架構決策：[ADR-0014](../../docs/adr/0014-native-cli-controller.md)（取代 ADR-0009 的 A1／B1；AI 暫定，待使用者確認）
- 詞彙：[GLOSSARY.md](../../GLOSSARY.md) 的 Controller、Controller CLI、Controller 帳號、Account Profile、桌面 App
- 前置：`platform-rebuild-v1` 已完成的 Runtime、MCP shim、Controller 工具（票 17、54）、Account Profile、額度觀測與排程

## Problem Statement

使用者實際用桌面 App 時遇到兩個問題：

1. **畫面難用**：主畫面把首次精靈、帳號、排程、跨登入、更新、共同設定、備份、恢復、平台狀態全部同時往下疊，選中 Workspace 的功能再接在最後；Workspace 的 12 個功能擠在左側清單以按鈕切換。找不到東西，也看不出現在在哪個功能。
2. **和 Controller 對話很難用**：現行對話頁是「打字 → Runtime 合併事件 → 每輪一次 `claude -p` → 前端輪詢」。使用者看不到 Controller 的工具呼叫過程，不能用 Claude Code 原生的指令、skills、快捷鍵，回應還要等。

而使用者在 `.scratch/platform-rebuild-v1` 已經跑順的方式是：自己開互動式 Claude Code 當主控，主控照一份執行規則，以其他帳號在背景派 `claude -p` Worker，背景程序結束時 Claude Code 原生通知主控，主控再派獨立驗證、修正，遇到不明確處自己決定並記到 `ai-decisions.md` 讓使用者事後審。使用者希望平台就用這種方式運作。

## Solution

- 桌面 App 改為**頁簽式**：頂層 `Workspace`｜`帳號`｜`設定`；Workspace 底下以頁簽一次只顯示一個功能。
- 新增 Workspace 的 **Controller 頁簽**：選一個 Account Profile 當 Controller 帳號，按「開啟」就在外部終端機視窗啟動原生互動式 Claude Code（Controller CLI）。它已經接好平台的 Controller 工具、載入內建的 Controller 操作手冊、工作目錄就是 Workspace。
- Controller 派出 Worker 後，以背景等待指令掛著；Worker 結束、受阻、裁定被回答或收到訊息時，等待指令結束，Claude Code 原生通知 Controller。
- 移除舊的對話頁與 Controller 自動喚醒。
- Controller 帳號在 CLI 開著時不被派去當 Worker。
- 可以把既有的 `~/.claude-Work-n` 目錄直接登記成 Account Profile，不必重新登入。
- Controller 與 Worker 自行做的決定，記在 Workspace 內的 AI 決策紀錄檔，格式與 `.scratch` 現行的 `ai-decisions.md` 相同。

## User Stories

### 頁簽與版面

1. As a 使用者, I want 頂層只有 `Workspace`、`帳號`、`設定` 三個頁簽, so that 我一眼知道平台的三大區塊。
2. As a 使用者, I want 主畫面一次只顯示一個功能, so that 不會被一長串疊在一起的設定淹沒。
3. As a 使用者, I want 選中 Workspace 後，主區顯示 Controller、工作、裁定、成果、審查與驗證、知識、流程、時間線、授權九個頁簽, so that 每個功能各自分開。
4. As a 使用者, I want 左側仍是 Workspace 清單, so that 我能快速在 Workspace 之間切換。
5. As a 使用者, I want 切換 Workspace 時停留在同一個頁簽, so that 比較不同 Workspace 的同一功能時不必重新點選。
6. As a 使用者, I want 新選的 Workspace 預設開在 Controller 頁簽, so that 我最常做的事（開 Controller）一步可達。
7. As a 使用者, I want 裁定頁簽與左側 Workspace 名稱旁顯示待裁定數徽章, so that 我不用切過去也知道有事要處理。
8. As a 使用者, I want 「審查與驗證」以小頁簽切換審查、驗證、探索, so that 相關品質功能放在一起又不互相干擾。
9. As a 使用者, I want 「知識」頁簽同時提供知識與專案理解, so that 背景資料集中在一處。
10. As a 使用者, I want 「授權」頁簽同時提供 Workspace 授權與模型覆寫, so that 會影響 Worker 能做什麼的設定集中在一處。
11. As a 使用者, I want `帳號` 頁簽提供帳號、排程與共同設定, so that 跟帳號與派工容量相關的設定集中管理。
12. As a 使用者, I want `設定` 頁簽提供跨登入啟動、更新、備份、恢復與平台狀態, so that 系統層級設定不會出現在日常工作畫面。
13. As a 使用者, I want 頂部狀態列、更新通知與完全退出進度一直顯示、不受頁簽影響, so that 重要狀態隨時看得到。
14. As a 新使用者, I want 沒有任何可用 Account Profile 時首次精靈以全畫面顯示, so that 我先完成必要設定才進入一般畫面。
15. As a 使用者, I want 重新開啟 App 時回到上次所在的頁簽與 Workspace, so that 不必每次重新導覽。
16. As a 使用者, I want 每個頁簽的標題層級、間距與按鈕位置一致, so that 畫面看起來整齊。
17. As a 使用者, I want 沒選 Workspace 時 Workspace 頁簽顯示「請從左側選擇或建立 Workspace」, so that 不會看到空白畫面。

### 開啟 Controller CLI

18. As a 使用者, I want 在 Controller 頁簽從可用的 Account Profile 中選一個當 Controller 帳號, so that 我決定哪個帳號負責協調。
19. As a 使用者, I want 下拉選單旁顯示每個 Profile 的 5h／7d 剩餘額度與目前執行中的 Worker 數, so that 我能挑一個適合的帳號。
20. As a 使用者, I want 不可用的 Profile（未登入、已封存、核對失敗）不能被選, so that 不會開出一個無法運作的 Controller。
21. As a 使用者, I want 按「開啟」就在外部終端機視窗啟動原生 Claude Code, so that 我用的是完整、原生的 Claude Code 體驗。
22. As a 使用者, I want 有 Windows Terminal 時用它開新視窗、沒有時用系統預設主控台, so that 任何 Windows 環境都能開。
23. As a 使用者, I want Controller CLI 的工作目錄是 Workspace 目錄, so that Workspace 自己的 `CLAUDE.md` 與檔案會被原生載入。
24. As a 使用者, I want Controller CLI 使用我採用的共同設定（plugins、MCP、指示）, so that 它跟我平常用的 Claude Code 行為一致。
25. As a 使用者, I want Controller CLI 一開就能使用平台的 Controller 工具（建立 Task、派工、取消、訊息、知識查詢、接受成果、提出裁定）, so that 我不用自己設定任何連線。
26. As a 使用者, I want Controller CLI 一開就載入內建的 Controller 操作手冊, so that Controller 知道平台的工作方式，不必我每次交代。
27. As a 使用者, I want Controller CLI 以 Claude Code 預設的權限模式（互動詢問）啟動, so that 我在場時保有確認權，需要時自己切換。
28. As a 使用者, I want Controller CLI 不繼承 App 或其他 Claude Code 程序的帳號相關環境變數, so that 它一定用我選的帳號。
29. As a 使用者, I want 同一個 Workspace 已有 Controller CLI 開著時，「開啟」變成「切到已開啟的視窗」或提示已開啟, so that 不會意外開出兩個 Controller。
30. As a 使用者, I want 同一個 Profile 可以同時當多個 Workspace 的 Controller 帳號, so that 我不必為每個 Workspace 準備不同帳號。
31. As a 使用者, I want 開啟失敗（找不到 `claude`、Profile 失效、終端機開不起來）時在頁簽上看到原因, so that 我知道怎麼修。

### Controller CLI 執行中

32. As a 使用者, I want Controller 頁簽顯示 CLI 狀態（未開啟、執行中、已結束、中斷）、Controller 帳號、開啟時間, so that 我知道 Controller 現在的狀況。
33. As a 使用者, I want Controller 頁簽持續顯示 Controller 帳號的 5h／7d 額度，接近上限時提示, so that 我能及早換帳號。
34. As a 使用者, I want Controller 帳號在 CLI 開著時不被派去執行 Worker, so that Controller 自己的互動額度不會被 Worker 吃光。
35. As a 使用者, I want CLI 結束後該 Profile 自動回到 Worker 派工候選, so that 帳號不會被閒置。
36. As a 使用者, I want 我在終端機關掉 Claude Code 時平台自動把 Controller 狀態改成已結束, so that 頁簽狀態永遠正確。
37. As a 使用者, I want 按「結束」可以從 App 結束 Controller CLI, so that 我不用找到那個視窗。
38. As a 使用者, I want 同帳號重新開啟時接續上次的原生 Session, so that 對話脈絡不會斷。
39. As a 使用者, I want 「換帳號重開」會結束目前 CLI 並以新帳號開新 Session, so that 額度用完時我能快速換手。
40. As a 使用者, I want 換帳號後的新 Controller 能從平台的 Task、裁定、成果與 AI 決策紀錄掌握進度, so that 換手不需要我重新說明。
41. As a 使用者, I want 時間線記錄 Controller CLI 的開啟、結束、中斷與帳號, so that 事後可以追查。

### 事件通知

42. As a Controller, I want 派工後在背景執行等待指令, so that 我可以繼續與使用者對話，Worker 有結果時再被通知。
43. As a Controller, I want 等待指令在 Worker Run 結束、Task 受阻、裁定被回答或收到訊息時結束並印出事件摘要, so that 我知道下一步該處理什麼。
44. As a Controller, I want 等待指令在啟動時已有未處理事件就立刻結束, so that 我不會漏掉在我沒掛等待時發生的事。
45. As a Controller, I want 同一個事件只被等待指令回報一次, so that 我不會重複處理。
46. As a Controller, I want 等待指令可以設定逾時並在逾時時以可辨識的方式結束, so that 我能決定要不要重新掛。
47. As a 使用者, I want 等待指令使用的憑證只從環境變數取得、不出現在命令列與 transcript, so that 憑證不外洩。
48. As a 使用者, I want CLI 結束後等待指令自動失效, so that 舊的背景程序不能再讀平台資料。

### 運作方式（Controller 操作手冊）

49. As a 使用者, I want Controller 依操作手冊從 Task 面板找出相依都已完成的 Task 依序處理, so that 工作以可預測的順序推進。
50. As a 使用者, I want Controller 以 `dispatch_task` 派工、由 Runtime 選帳號, so that 額度門檻與並行上限由平台統一執行。
51. As a 使用者, I want 帳號額度門檻（5h、7d 剩餘 ≥ 3%）與每帳號並行上限由 Runtime 的排程設定執行、不寫在提示裡, so that 規則改了只要改設定。
52. As a 使用者, I want Controller 在實作 Task 完成後派一個不同 Worker 做唯讀驗證, so that 成果有獨立確認。
53. As a 使用者, I want 驗證不通過時 Controller 派修正 Task，最多 3 次，超過就把 Task 標為受阻並寫明原因後處理下一個, so that 不會無限重試。
54. As a 使用者, I want Controller 遇到不明確處先自己決定並繼續，只有決策權限表規定要問我的才提出 Decision Request, so that 工作不會一直停下來等我。
55. As a 使用者, I want Controller 與 Worker 把自行做的決定追加到 Workspace 的 AI 決策紀錄檔，附重要度（高／中／低）與「審閱：未審」行, so that 我能事後依重要度審閱。
56. As a 使用者, I want AI 決策紀錄的格式與 `.scratch` 現行的 `ai-decisions.md` 相同, so that 我能沿用 review-ai-decisions skill。
57. As a 使用者, I want Worker 只能追加 AI 決策條目、不能修改既有條目, so that 紀錄可信。
58. As a 使用者, I want Workspace 的 `CLAUDE.md` 可以補充或收窄操作手冊, so that 每個 Workspace 能有自己的工作習慣。
59. As a 使用者, I want 操作手冊不能放寬 Workspace 授權, so that 安全邊界不會被提示文字繞過。
60. As a 使用者, I want 操作手冊隨 App 發佈並有版本號，Controller 頁簽顯示目前版本, so that 我知道 Controller 照哪一版規則工作。

### Worker 指示

61. As a 使用者, I want Worker 收到的指示由 Task 的六項最小契約加上 Runtime 固定附加的 Worker 通則組成, so that 不必另外維護一套範本。
62. As a 使用者, I want Worker 通則要求以 TDD 實作、commit 到該 Task 的 worktree 分支、最多同時 3 個 subagent、自行決定的事項寫入 AI 決策紀錄, so that Worker 的工作方式與 `.scratch` 一致。
63. As a 使用者, I want 驗證與修正只是不同契約的 Task, so that 平台不需要額外的驗證流程概念。

### Account Profile

64. As a 使用者, I want 在帳號頁簽「以既有目錄建立 Profile」，選擇 `~/.claude-Work-n` 這類已登入的設定目錄, so that 不必每個帳號重新登入。
65. As a 使用者, I want 以既有目錄建立的 Profile 立即核對登入身分, so that 我知道它是不是我以為的那個帳號。
66. As a 使用者, I want 平台拒絕與其他 Profile 重疊（相同或互相包含）的目錄, so that 兩個 Profile 不會共用登入狀態。
67. As a 使用者, I want 封存以既有目錄建立的 Profile 時平台不刪除該目錄, so that 我平台外的使用不受影響。

### 退出、當掉與恢復

68. As a 使用者, I want 只關閉 App 視窗（Runtime 仍在）時 Controller CLI 不受影響, so that 我可以把 App 收到系統匣繼續工作。
69. As a 使用者, I want 完全退出時 Runtime 結束 Controller CLI，退出進度列出這一項, so that 不會留下工具已失效的 Controller。
70. As a 使用者, I want Runtime 當掉恢復後，原 Controller Run 標為中斷、舊憑證失效，Controller 頁簽提示重新開啟, so that 我知道要重開。
71. As a 使用者, I want 恢復後重新開啟時可用同帳號接續原生 Session, so that 對話脈絡保留。

### 移除舊對話

72. As a 使用者, I want 舊的「對話」頁簽被移除, so that 只有一種和 Controller 互動的方式。
73. As a 使用者, I want Runtime 不再因使用者訊息或工作事件自動啟動 `claude -p` 的 Controller 輪次, so that 不會在背景消耗額度。
74. As a 使用者, I want 舊的對話資料保留在資料庫、不遷移也不再寫入, so that 移除功能不會遺失歷史。

## Implementation Decisions

### 架構

- 依 ADR-0014：Controller 是原生互動式 Claude Code，開在外部終端機視窗；Runtime 仍是唯一控制平面與唯一寫入者。

### Controller CLI 生命週期（Runtime）

- 新增 Domain 概念「互動式 Controller Run」：Run 的一種，綁 Workspace、Controller 帳號（Account Profile）、程序識別、Agent 憑證與開啟時間；狀態為 `starting`、`running`、`ended`、`interrupted`。每個 Workspace 同時最多一個非終態的 Controller Run。
- 開啟流程沿用 launch intent：先持久化啟動意圖（含 `command_id`），再請 Port 開終端機；Runtime 在回執前當掉時以 `command_id` 找回，不重複開窗。
- 啟動內容與 Worker 的 Claude Code Adapter 共用同一套組裝：清除繼承的 `CLAUDE*`、`CLAUDECODE`、`ANTHROPIC*`、`AI_AGENT`、`BAGGAGE`；`CLAUDE_CONFIG_DIR` 指向 Controller 帳號；`--strict-mcp-config --mcp-config` 指向 MCP shim 與共同設定的 MCP 伺服器；設定快照；`--append-system-prompt-file` 指向操作手冊；工作目錄是 Workspace。差別只在：不加 `-p`、不指定輸出格式、不指定權限模式、Agent 憑證範圍是 Controller。
- 同帳號重開時加 `--continue`，接回該 Workspace 目錄在該帳號下的最近原生 Session；換帳號不接續。
- 終端機內執行的不是 `claude` 本身，而是 MCP shim 執行檔的「Controller 宿主」子命令：它以繼承主控台的方式啟動 `claude`，啟動時與結束時向 Agent API 回報（含 `claude` 的程序識別與結束碼）。宿主被強制關閉時，Runtime 以既有的程序存活查驗發現並把 Run 標為 `ended`。這讓 Runtime 不必是終端機程序的父程序也能追蹤生命週期。
- 結束：使用者按「結束」或完全退出時，Runtime 透過程序控制結束宿主與其子程序。Run 結束即撤銷憑證。
- Runtime 恢復時，所有非終態的 Controller Run 先查驗程序；仍存活者重新綁定（憑證世代加一，舊憑證失效，提示使用者重開以取得新憑證）；不存活者標為 `interrupted`。

### Port

- 擴充現有的 Claude 環境 Port（已有「開登入終端機」）：新增「開啟 Controller 終端機」，輸入為完整的啟動規格（執行檔、參數、環境變數增刪、工作目錄、視窗標題），輸出為宿主程序識別。真實實作優先 Windows Terminal，否則系統預設主控台。
- harness 的 fake 版記錄每次開窗的啟動規格，並能模擬宿主回報、宿主消失、開窗失敗。

### 帳號排除

- Scheduler 的帳號候選條件新增：該 Profile 沒有非終態的 Controller Run。已在執行的 Worker 不受影響（開啟 Controller 不會中斷既有 Worker）。

### 事件等待

- Agent API 新增 Controller 專用的事件等待端點：長輪詢，參數為「已確認的事件游標」與逾時；有游標之後的事件就立即回傳，否則等到有事件或逾時。事件種類：Worker Run 結束、Task 受阻、Decision Request 被裁定、Controller 收到訊息。游標由 Runtime 依 Controller Run 保存，確認後同一事件不再回傳。
- MCP shim 執行檔新增 `wait` 子命令：從環境變數取得憑證與 Runtime 位址，呼叫等待端點，有事件就以人類與模型都易讀的格式印出摘要並以 0 結束；逾時以另一個固定結束碼結束；憑證失效以第三個結束碼結束。
- 這個子命令不做判斷、不保存狀態，維持 shim「只轉送」的定位。

### Controller 操作手冊

- 一份隨 App 發佈、有版本號的唯讀文件，內容為 `execution.md` 的平台化版本：frontier 選擇、`dispatch_task` 派工、掛背景 `wait`、獨立驗證、最多 3 次修正、先決定再記錄、只有決策權限表（D-15）規定的事項才提 Decision Request、AI 決策紀錄的位置與格式。
- 不包含帳號名稱、額度門檻、並行上限、worktree 路徑（這些由 Runtime 負責）。
- Runtime 以設定快照相同機制把它放進 Run 專屬的唯讀目錄後傳給 CLI。

### Worker 通則

- Claude Code Adapter 為 Worker 附加的系統提示加入固定的 Worker 通則：TDD、commit 到 Task 的 worktree 分支、最多 3 個 subagent、AI 決策只能追加。

### AI 決策紀錄

- 位置：Workspace 目錄下的 `.baton/ai-decisions.md`；不存在時由 Runtime 在開啟 Controller CLI 前以標頭範本建立。
- 格式與 `.scratch/platform-rebuild-v1/ai-decisions.md` 相同（`### AI-NN：主題`、重要度、時間／階段、決策、理由、替代方案、影響、`- 審閱：未審`）。
- 平台不解析、不索引、不顯示這個檔案（範圍外）；它就是給 Agent 寫、給使用者與 review-ai-decisions skill 讀的檔案。

### Account Profile 以既有目錄建立

- 管理 API 新增「以既有目錄建立 Profile」：輸入目錄路徑與名稱；驗證目錄存在、與其他 Profile 不相同也不互相包含、不在平台資料目錄的 profiles 底下；建立後立即核對登入身分。
- Profile 記錄「目錄由平台建立」或「外部目錄」；封存外部目錄的 Profile 不刪除目錄。

### 移除舊對話

- 移除對話頁、對話相關的前端 client 方法與 Runtime 的 Controller 自動喚醒（事件合併、`claude -p` 輪次、Controller 自動交接）。保留資料表，不遷移、不再寫入。
- Controller 工具（票 17、54）與 MCP shim 不變。

### 桌面 UI

- App 外殼改為頂層頁簽（Workspace／帳號／設定）＋ Workspace 二層頁簽；左側 Workspace 清單改為只負責選擇 Workspace，不再列功能按鈕。
- 現有各功能元件原樣放進對應頁簽，只調整外層容器、標題層級與間距；不改元件內部行為。
- 目前所在頁簽與 Workspace 存在本機（每個檢視者的偏好），讀不到時預設 Workspace／Controller。
- 新增 Controller 頁簽元件：Profile 選擇（附額度與執行中 Worker 數）、開啟／結束／換帳號重開、狀態、開啟時間、操作手冊版本、錯誤原因。
- 帳號頁簽新增「以既有目錄建立 Profile」。

## Testing Decisions

- **好測試只斷言外部行為**：API 回應、可觀察狀態、時間線事件、Port 被呼叫的方式（例如「開窗的啟動規格帶了哪個 `CLAUDE_CONFIG_DIR`」「沒有第二次開窗」），不斷言資料表欄位或內部呼叫順序（ADR-0012）。
- **主要 seam：Runtime harness**（沿用，不新增 seam）。只新增一個 fake 能力：Claude 環境 Port 的「開啟 Controller 終端機」。以 harness 驗證：
  - 開啟會建立 Controller Run、啟動規格正確（環境清理、帳號目錄、MCP 設定、操作手冊、工作目錄、無 `-p`、同帳號重開帶 `--continue`）。
  - 同一 Workspace 第二次開啟被拒；同一 Profile 可開兩個 Workspace。
  - 開啟期間 Scheduler 不把 Controller 帳號分給 Worker；結束後恢復；既有 Worker 不受影響。
  - 宿主回報結束、宿主消失、使用者按結束、完全退出，都讓 Run 結束並撤銷憑證。
  - 在啟動意圖與回執之間崩潰後恢復，不重複開窗；恢復後存活／不存活的處理。
  - 等待端點：已有事件立即回傳、無事件等到事件或逾時（fake Clock）、確認後不重複、憑證失效被拒。
  - 不再產生任何自動 Controller 輪次。
  - 以既有目錄建立 Profile 的驗證與封存不刪目錄。
- **MCP shim `wait` 子命令**：沿用 shim 現有的測試方式，只測結束碼與輸出格式對應（有事件、逾時、憑證失效）。
- **UI**：沿用各元件旁的 Vitest 元件測試（以 fixtures 的假 client）測 Controller 頁簽與 App 外殼的頁簽切換、記住所在頁簽、徽章；沿用票 53 的 fake Port E2E 組態補一條「選 Workspace → Controller 頁簽 → 開啟 → 狀態變執行中 → 結束」旅程。
- **手動驗收**：真實 Windows 上開 Windows Terminal／預設主控台、原生 `claude` 互動、背景 `wait` 收到通知、完全退出關窗，補進 Windows 平台驗收清單。

## Out of Scope

- 內嵌終端機（ConPTY＋xterm.js）。
- Controller 自動交接、跨帳號接續原生 Session。
- AI 決策紀錄的平台化（解析、索引、審閱頁）。
- UI 配色、字體、元件庫等視覺重設計；只做資訊架構與一致的版面。
- 可編輯的 Worker 指示範本、可編輯的操作手冊（Workspace 用 `CLAUDE.md` 補充即可）。
- 舊對話資料的遷移或清除。
- 非 Windows 平台的終端機開啟。

## Further Notes

- ADR-0014 與 grilling.md 的決策目前都是 AI 暫定。使用者若修改其中任何一題，請同步回本 spec 對應段落。
- 使用者若同時在平台外以同一個 `.claude-Work-n` 跑 `claude -p`（例如 `.scratch` 流程），平台的並行計數看不到那些程序；額度觀測仍反映真實用量。
- Workspace 不一定是 Git repo，`.baton/ai-decisions.md` 不會自動版控。
- 「等待」依賴 Controller 遵守操作手冊掛上背景指令；沒掛時事件只會累積，下次掛上時立即回傳。
