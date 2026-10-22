# Spec：通用多 Agent 協作平台 v1（Baton）

Status: ready-for-agent
Source: GitHub issue Anuise/issue-orchestrator#3（v0.4 技術設計與驗收規範，SQLite 方案）
Date: 2026-10-22

> 本 spec 以 issue #3 的 v0.4 設計為權威基準，已確認的 DR-20261021-01～08、DR-20261022-01～03 全數沿用，不重新討論。
> issue 中標示「待定」「未決」或未說明的地方，由 Claude 代為決定，並以 **【Claude 決策 D-NN】** 標註。這些決策都是「先用起來再依實際使用感受修正」的暫定值；修改時只需更新本檔對應條目與「Claude 決策索引」，不需重寫其他段落。
> 標示 **Validation Required** 的項目沿用 issue 原意：政策已定，但 Windows 原生行為須實測後才能宣稱成立。

---

## Claude 決策索引

| ID | 主題 | 決策摘要 | 改動成本 |
| --- | --- | --- | --- |
| D-01 | 產品與 repo 名稱 | 暫定產品名 **Baton**（指揮棒，呼應 Controller 與交接棒），repo 改名延後到第一階段可運作後再執行 | 低 |
| D-02 | 本 spec 範圍與交付順序 | 一份 spec 涵蓋第一版完整範圍，依 issue §15 四階段切 ticket；第一、二階段構成可自用的 MVP | 中 |
| D-03 | 舊 skill 的去留 | 重建期間保留現有 `skills/` 不動；以 git tag 封存舊版；刪除與改名列為最後一張 ready-for-human ticket | 低 |
| D-04 | 主要測試 seam | 以「Runtime API + 注入式 Port（Adapter／Clock／QuotaObserver／ProcessControl）」為唯一主要 seam | 高 |
| D-05 | Agent 與 Runtime 的工具介面 | Runtime 提供本機 MCP 工具伺服器（stdio shim），每個 Run 注入專屬 MCP 設定與範圍憑證 | 高 |
| D-06 | Controller 執行方式 | 每一輪 Controller 判斷為一次 `claude -p` 呼叫，以原生 Session resume 接續長期對話 | 中 |
| D-07 | Worker 執行方式 | 一個 Run 對應一次 `claude -p` 程序；以 MCP 工具回報 checkpoint、訊息與成果 | 中 |
| D-08 | 權限轉接 | Worker 以非互動權限模式執行，權限提示經 permission-prompt MCP 工具轉給 Runtime 依授權判定或建立裁定 | 中 |
| D-09 | S3 設定版本固定 | 每個 Run 啟動時把設定快照寫成該 Run 專屬、唯讀的設定檔再傳給 CLI；原生 Profile 目錄只放登入狀態 | 中 |
| D-10 | 程序控制 | 每個 Run 一個 Windows Job Object；程序身分 = PID + 建立時間 + 指令雜湊 | 中 |
| D-11 | 完全退出寬限期 | 30 秒受控停止 + 10 秒強制終止確認，總上限 40 秒 | 低 |
| D-12 | 額度觀測來源 | 以 CLI 串流中的限流事件為「已觀測」、以本機用量累計推估為「推估」，兩者並存並標明可信度 | 中 |
| D-13 | 交接門檻與遲滯 | 推估用量 ≥75% 停止新派工、≥85% 開始 draining；<60% 或重置後才恢復可派；同 Task 換帳號冷卻 10 分鐘 | 低 |
| D-14 | 並行容量預設 | 每帳號 2 個並行 Run、全域 6 個；帳號數 ≥3 時保留 1 個交接槽；使用者可調 | 低 |
| D-15 | 人工裁定權限表 | 明列 Controller 可自主處理與必須送裁定的事項（見實作決策） | 中 |
| D-16 | 修正與重試預算 | 實作失敗 3 次、驗證不完整 2 次、基礎設施重試 5 次（指數退避），各自獨立計數 | 低 |
| D-17 | 本機資料位置 | 全部放在 `%LOCALAPPDATA%\Baton\`（控制資料庫、成果、Profile、備份、日誌分子目錄） | 低 |
| D-18 | 備份政策 | 每日一次 + migration 前 + 重要裁定／階段完成後（每小時最多一次）；保留 7 份每日 + 4 份每週；每週自動還原演練 | 低 |
| D-19 | 安裝與更新 | Tauri NSIS 單使用者安裝（免系統管理員）、簽章自動更新；只提示、不自動套用 | 中 |
| D-20 | Claude Code 相依檢查 | 首次啟動精靈只檢查與引導（PATH、版本、Git for Windows、各 Profile 登入），不代為下載安裝 | 低 |
| D-21 | Web UI 版面 | 左側 Workspace 導覽 + 主區分頁 + 右側詳細面板；裁定中心以徽章顯示待處理數 | 低 |
| D-22 | 本機憑證格式與交付 | 256-bit 隨機 bearer token，資料庫只存雜湊；桌面憑證以 DPAPI 保護檔交付；Agent 憑證只走環境變數 | 中 |
| D-23 | 網頁存取防護 | 必帶自訂標頭 + Host／Origin 白名單 + 拒絕所有 CORS 預檢；串流同樣驗證 | 低 |
| D-24 | 監護與 Runtime 的銜接 | 監護只寫獨立的 append-only 日誌檔（每筆 fsync），Runtime 啟動時匯入控制資料庫 | 中 |
| D-25 | 跨登入啟動入口 | HKCU Run 登錄機碼啟動輕量 launcher；停止意圖另以原子寫入的生命週期檔鏡像，供 launcher 不開資料庫即可判斷 | 中 |
| D-26 | 全文檢索 | SQLite FTS5 + trigram tokenizer（支援繁體中文子字串搜尋） | 低 |
| D-27 | Worker 間訊息交付 | Worker 在 checkpoint 與工具呼叫間透過 MCP 工具拉取收件匣；Runtime 不中途插話 | 低 |
| D-28 | 教訓升級 | 候選教訓須使用者在「知識」頁確認才升為已驗證；Guard 一律由使用者啟用 | 低 |
| D-29 | 驗證 Adapter 範圍 | 第一版只做 CLI／API 與 web（Playwright 為基礎）；mobile 延後 | 中 |
| D-30 | 模型選擇 | 平台預設不指定模型，沿用各帳號 Claude Code 預設；Workspace 可依角色覆寫 | 低 |
| D-31 | 計時參數 | heartbeat 15 秒、lease 60 秒、Controller 事件合併視窗 5 秒、額度輪詢 5 分鐘 | 低 |
| D-32 | 帳號數上限 | 第一版支援最多 8 個 account_profile | 低 |
| D-33 | 時區與顯示 | 一律以 Windows 系統時區顯示，資料庫存 UTC | 低 |

---

## Problem Statement

我同時擁有多個 Claude Code 帳號，想把「長時間、多步驟、可能跨研究／文件／程式的工作」交給 Agent 自主推進，而不是自己守在終端機前面。現有的 issue-orchestrator 是一個 skill：單一 Worker、共用 checkout、綁定本地 Issue、所有狀態寫在 Markdown 與 JSON 檔，而且只活在一次 Claude Code 對話裡。實際使用時遇到這些痛點：

- 一個帳號的五小時或週用量用完，整個工作就停住；我得自己發現、自己換帳號、自己把進度交代給新的對話。
- 對話被壓縮、Claude Code 當掉或電腦重開機後，進度、決策和「為什麼這樣做」散落在檔案與記憶裡，接手的 Agent 常常重做或做錯。
- Agent 遇到需要我決定的事情，只能停下來等我在那個終端機回覆；我不在電腦前時，整個流程就卡住，而且之後很難一次看到所有待決事項。
- 同樣的錯誤會在不同工作、不同對話裡反覆出現，因為教訓沒有被結構化保存，更沒有變成可執行的防護。
- 只能循序做事；明明有多個帳號和獨立的工作，卻不能並行。
- 工作來源被綁死在「spec + issues」與程式開發，研究、文件這類工作無法用同一套方式管理。

## Solution

把 repo 重建為一個 Windows 桌面應用程式 **Baton**【D-01】：

- 我在桌面 App 裡跟一個長期存在的 **Controller** 對話、討論目標。Controller 本身是 Claude Code，負責釐清目標、拆 Task、派工、協調與評估成果；大量實作與研究交給 **Worker**。
- 一個常駐的 **Runtime**（Rust）管理所有狀態（SQLite）、帳號、程序生命週期、排程、交接與恢復。關掉視窗工作照常進行；系統匣可以重新開啟或「完全退出」。
- 多個 Claude Code 帳號同時執行；接近用量上限時，Runtime 主動把工作交接給其他有餘額的帳號，並從檢查點延續，不消耗修正預算；全部帳號受限時保存進度、等待重置。
- 需要我決定的事情進入 **裁定中心**，我有空時打開集中處理；裁定後受影響的工作自動恢復，不相關的工作從頭到尾不受阻擋。
- 決策、教訓與防護被保存下來，在相關工作開始前載入或檢查。
- 程式開發 Task 各自使用獨立 worktree，完成並整合後自動清理；研究與文件 Task 使用獨立輸出目錄。
- Controller 以版本化的工程方法（Playbook、Stage／Gate、雙軸 Review、真實介面驗證）推進工作，Worker 宣稱成功不等於完成。
- Runtime 意外崩潰時，監護程序有限次數自動重啟並安全恢復；所有恢復都「先核對、再恢復」，不盲目重派或重送外部操作。

## User Stories

### 安裝與首次啟動

1. As a Windows 11 使用者, I want 下載一個安裝檔就能裝好 Baton 而不需要系統管理員權限或自行安裝 Node.js／Rust／Python／資料庫, so that 我能在幾分鐘內開始使用。【D-19】
2. As a 新使用者, I want 首次啟動時有精靈檢查 Claude Code 是否已安裝、版本是否符合、Git for Windows 是否存在, so that 我知道還缺什麼、該怎麼補。【D-20】
3. As a 新使用者, I want 精靈顯示官方安裝指令並提供「重新檢查」按鈕而不是替我執行下載腳本, so that 我能自行決定要在系統上執行什麼。【D-20】
4. As a 使用者, I want 新增多個 account_profile 並為每一個完成原生 Claude Code 登入, so that 不同帳號的登入與 Session 狀態互不干擾。
5. As a 使用者, I want 每個 Profile 新增後 Baton 立即核對實際認證來源與帳號身分, so that 我不會因為環境變數或 credential helper 而以為在用 A 帳號、實際卻在用 B 帳號。
6. As a 使用者, I want 從我平常使用的 Claude Code 設定匯入共同基準（偏好、指示、hooks、MCP、plugins），並在採用前看到來源與差異, so that 所有帳號的 Agent 行為一致，而我平常手動使用的設定不會被改寫。
7. As a 使用者, I want 首次使用時被清楚告知「關閉視窗後工作會在背景繼續、請由系統匣完全退出」, so that 我不會誤以為關掉視窗就停止了所有工作。
8. As a 使用者, I want App 顯示目前的 Windows 版本、版本別、原生架構與平台支援狀態, so that 我知道自己是否在正式支援範圍內。
9. As a 在 ARM 或 Windows 10 上的使用者, I want 被明確告知不屬正式支援, so that 我不會把問題誤認為產品缺陷。

### Workspace 與授權

10. As a 使用者, I want 指定一個目錄建立 Workspace，而且該目錄可以不是 Git repo, so that 研究與文件工作也能用同一套平台。
11. As a 使用者, I want 建立 Workspace 時看到可編輯的授權範本（能力、目標資源、限制），確認後才生效, so that 選取目錄不會被解讀成授權 Agent 做任何事。
12. As a 開發者, I want 套用「可信專案開發（P3）」範本，涵蓋編輯工作副本、在指定環境建置測試、從指定來源安裝專案依賴, so that Worker 不會在每一步都停下來要批准。
13. As a 使用者, I want 確認摘要中明確揭露「專案與第三方依賴程式會以我的 Windows 身分執行」的風險, so that 我在知情下授權。
14. As a 使用者, I want 管理員安裝、全域安裝、遠端寫入、部署、對外發訊、付費與正式資料操作各自需要明確授權, so that 開發範本不會偷偷涵蓋高風險操作。
15. As a 使用者, I want 在授權介面中區分「允許本次操作」與「修改 Workspace 授權」, so that 一次性批准不會變成永久擴權。
16. As a 使用者, I want 撤銷授權後看到「後續執行已停用，仍有 N 個 Run 沿用舊授權」以及這些 Run 的暫停／取消入口, so that 我理解撤權不是立即生效並能自行介入。
17. As a 使用者, I want 每個 Workspace 有自己的「完全退出後自動續跑」與「跨登入自動續跑」設定且預設關閉, so that 一個 Workspace 的設定不會連帶啟動其他工作。

### 與 Controller 對話與目標

18. As a 使用者, I want 在 Workspace 裡跟 Controller 長期對話，即使 Controller 換帳號、換 Session 或重啟，對話仍是同一個, so that 我不用重講背景。
19. As a 使用者, I want 在 Controller 忙碌時送出的訊息顯示「已收到／待處理」並在本輪結束後依序處理, so that 我知道訊息沒有遺失，也不會打斷進行中的操作。
20. As a 使用者, I want 暫停與取消按鈕立即生效而不用排在 Controller 對話佇列之後, so that 我能隨時止損。
21. As a 使用者, I want 提出模糊目標時 Controller 標示已確認事項、假設與缺口，只先問會阻擋下一步的關鍵問題, so that 工作能邊釐清邊推進。
22. As a 使用者, I want 不依賴未決答案的研究先被派出，依賴答案的實作保持等待, so that 我的回覆不會成為全部工作的瓶頸。
23. As a 使用者, I want 我的回答改變既有假設時，受影響的 Task 與成果被標示重驗，而獨立工作繼續, so that 舊成果不會在錯誤前提下被接受。
24. As a 使用者, I want 在沒有新事件時 Controller 不被反覆喚醒巡查, so that 用量不會浪費在空轉。
25. As a 使用者, I want 隨時補充方向、調整 Task 優先序, so that 計畫能跟著我的判斷變動。

### Task、派工與 Worker

26. As a 使用者, I want 每個 Task 都有六項最小契約（目標、輸入與依據、範圍與限制、預期成果、完成與驗證條件、阻礙處理）而由 Controller 整理, so that Worker 清楚知道要做什麼、做到哪裡算完成。
27. As a 使用者, I want 契約不完整的 Task 不會被派工，並顯示缺少哪些內容, so that 不會有 Worker 在模糊委託下亂做。
28. As a 使用者, I want 在工作面板看到每個 Task 的狀態、目前執行者、帳號、等待原因、最後有效進度與下一步, so that 我一眼掌握全局。
29. As a 使用者, I want 不同帳號的 Worker 並行處理互不相依的 Task, so that 多帳號真的帶來產能。
30. As a 使用者, I want 同一個 Task 不會同時被兩個 Worker 取得有效寫入所有權, so that 不會互相覆寫成果。
31. As a 開發者, I want 每個開發 Task 在獨立 worktree 中執行、研究與文件 Task 在獨立輸出目錄中執行, so that 並行修改不會衝突。
32. As a 開發者, I want 程式分支的整合排入單一整合佇列並在合併後驗證整合結果, so that 主線不會被並行合併弄壞。
33. As a Worker, I want 透過工具 API 與其他 Worker 交換研究結果或協商介面, so that 相關工作能協同，但正式範圍變更仍交給 Controller。
34. As a Worker, I want 在缺資料、失敗、預算不足或需求衝突時有明確的回報管道, so that 我不需要自行擴權或改寫完成標準。
35. As a 使用者, I want Worker 宣稱成功、程序正常退出都不等於 Task 完成, so that 只有通過驗證條件的成果才會被接受。
36. As a 使用者, I want Task 依實作失敗、驗證不完整與資源中斷分開計數, so that 額度交接不會吃掉修正預算。【D-16】
37. As a 使用者, I want Task 完成且成果整合後 worktree 被自動清理，並看到「已完成」與「已清理／待清理／清理失敗」分開顯示, so that 磁碟空間被回收但不會誤刪未保存的成果。
38. As a 使用者, I want 暫停、交接、失敗、取消與完全退出都不會觸發 worktree 自動清理, so that 仍需恢復的工作現場不會消失。
39. As a 使用者, I want 清理失敗（例如 Windows 檔案占用）時保留待清理狀態並有限重試、顯示可行動原因, so that 我知道要關掉哪個程式。

### 帳號額度與交接

40. As a 使用者, I want 在帳號頁面看到每個 Profile 的五小時與週用量、觀測時間、重置時間與可信度（已觀測／推估／未知）, so that 我知道數字可以信到什麼程度。【D-12】
41. As a 使用者, I want 帳號接近上限時 Runtime 自動停止對它派新工作並把進行中的工作交接給其他有餘額的帳號, so that 工作不會因為額度而停住。【D-13】
42. As a 使用者, I want 交接後的 Worker 從最近有效檢查點、決策與實際成果延續，並確認已完成、未完成與未知副作用, so that 不會重做或漏做。
43. As a 使用者, I want 舊 Worker 確認停止之前新 Worker 不能進入同一可寫目錄, so that 不會有兩個 writer 同時寫。
44. As a 使用者, I want Controller 本身也能在額度接近上限時交接到其他帳號, so that 協調工作不會因為 Controller 帳號用完而中斷。
45. As a 使用者, I want 帳號突然被限流時，Runtime 從最近有效檢查點與實際成果重建，不依賴最後一輪摘要成功, so that 突發限流也能恢復。
46. As a 使用者, I want 所有合適帳號都受限時，工作進入 waiting_resource、依重置時間安排檢查、並顯示等待原因, so that 我知道何時會自動恢復。
47. As a 使用者, I want 選擇較高並行或保留較多交接容量, so that 能依當下需求取捨。【D-14】
48. As a 使用者, I want 額度不足與授權不足、相依未完成、狀態不明等阻擋原因分開呈現, so that 我知道該處理的是哪一類問題。

### 裁定中心

49. As a 使用者, I want 在有空的時候打開裁定中心，先看總覽再逐項處理, so that 我能集中、非同步地做決定。
50. As a 使用者, I want 每個裁定請求包含問題、為什麼 Agent 無法自行決定、選項、取捨、Controller 建議、證據、阻擋範圍、緊急程度與期限, so that 我能快速做出有依據的決定。
51. As a 使用者, I want 對每一項選擇接受方案、修改條件、回答、要求調查、延後或駁回, so that 處理方式有彈性。
52. As a 使用者, I want 批次處理多項裁定時，每一項仍各自保存對應關係, so that 不同問題不會被當成同一份概括授權。
53. As a 使用者, I want 相似的問題被合併或互相連結, so that 不會被重複詢問。
54. As a 使用者, I want 隔天才打開裁定中心時問題仍完整，而目標版本已改變的請求被標記為已過期並重新整理, so that 我不會批准到過期的內容。
55. As a 使用者, I want 裁定後受影響的工作自動恢復，而逾時不會被視為同意, so that 我的沉默不會變成授權。
56. As a 使用者, I want 明確知道哪些事情 Controller 可以自行決定、哪些一定會問我, so that 我能信任它的自主範圍。【D-15】
57. As a 使用者, I want 只有我透過桌面 App 提交的裁定才算人工正式裁定, so that Agent 不能冒充我批准。

### 知識、教訓與防護

58. As a 使用者, I want 決策、教訓與知識依全域、Workspace、Goal、Task 範圍保存並附來源、證據與有效期間, so that Agent 拿到的是適用且可追溯的資訊。
59. As a 使用者, I want 候選教訓由我確認後才升為已驗證，錯誤教訓可以撤銷並保留歷史, so that 錯誤經驗不會被當成規則。【D-28】
60. As a 使用者, I want 可機械化的教訓轉為啟動前檢查、測試或重試限制等 Guard, so that 同樣的錯誤被程式攔住而不只是被提醒。
61. As a 使用者, I want 每個 Run 都有 context receipt 記錄載入了哪些決策、教訓與版本、因預算省略了什麼, so that 出錯時能追查 Worker 當時知道什麼。
62. As a 使用者, I want 用繁體中文關鍵字搜尋決策與教訓, so that 我能找到過去的判斷。【D-26】

### 工程方法與驗證

63. As a 使用者, I want Workspace 首次接入專案時 Controller 建立附來源的 Project Brief、Glossary 與 Project Map，未確認的關係標為假說, so that 後續工作有共同理解。
64. As a 使用者, I want 功能、缺陷、調查、架構遷移、效能、探索測試走各自適用的 Playbook，研究與文件工作不被強制套用開發流程, so that 方法符合工作性質。
65. As a 使用者, I want 每個 Stage 依證據推進、跳過時保存原因，且不能跳過我明定的裁定或安全門檻, so that 流程不會被文字宣告「做完了」繞過。
66. As a 使用者, I want Review 分 Standards 與 Spec 兩軸保存，任一軸不通過 Gate 都保持關閉, so that 程式品質好不能掩蓋需求缺漏，反之亦然。
67. As a 使用者, I want 高風險工作需要非作者的獨立 reviewer，缺席時保持阻擋, so that 不會用自我審查冒充獨立審查。
68. As a 使用者, I want 實作偏離設計時保存偏離、原因與接受者，接受後更新設計版本, so that 下一位 Worker 拿到的是正確的設計。
69. As a 使用者, I want 專案有 Verification Profile 描述如何啟動、就緒、操作、斷言與清理真實介面, so that 驗證檢查的是真實行為而不是 Worker 的說法。【D-29】
70. As a 使用者, I want 驗證步驟區分 passed、product_failure、inconclusive、environment_blocked、auth_blocked、budget_exhausted、automation_unsupported, so that 證據不足或環境故障不會被算成通過或產品缺陷。
71. As a 使用者, I want 探索測試找到的疑點經重現才升級為 confirmed defect，修復後納入正常回歸, so that 缺陷清單可信。
72. As a 使用者, I want 只有被後續斷言確認過的動作才會被錄製重播，且 no_live_model 模式下保證不呼叫模型, so that 重播不會冒充驗收。
73. As a 使用者, I want 需求、build 或驗證 Profile 改變時受影響的 Gate 自動失效並建立重驗工作, so that 舊的通過結果不會被沿用到新版本。
74. As a 使用者, I want Playbook 升級不會靜默改變進行中的 Workflow, so that 進行中的工作規則穩定。

### 生命週期、恢復與可靠性

75. As a 使用者, I want 關閉視窗後工作繼續，重新開啟時連回同一個 Runtime 且不重複啟動任何 Worker, so that 我可以隨意開關視窗。
76. As a 使用者, I want 從系統匣「完全退出」時看到停止進度，40 秒內所有平台管理的程序都停止或被明確標示異常, so that 退出是有界且可信的。【D-11】
77. As a 使用者, I want 完全退出後佇列重試、額度恢復與交接都不能再啟動新的 Worker, so that 我的停止意圖被尊重。
78. As a 使用者, I want 完全退出後再次開啟時先核對狀態，再依各 Workspace 設定自動續跑或等我按「繼續工作」, so that 恢復行為可預期。
79. As a 使用者, I want Runtime 意外崩潰時監護程序自動重啟它並安全恢復工作，同一事件最多三次、退避 30 秒與 2 分鐘、穩定 10 分鐘才算成功, so that 偶發故障不需要我介入，連續故障也不會無限循環。
80. As a 使用者, I want 三次恢復都失敗後看到診斷與「重新嘗試恢復」入口，重開 App 不會偷偷產生第四次, so that 我掌握是否要再試。
81. As a 使用者, I want 監護程序自己掛掉時工作繼續並提示「工作繼續，自動重啟保護不可用」, so that 我知道保護降級但工作沒有中斷。
82. As a 使用者, I want 可選擇登入 Windows 時自動啟動 Baton（預設關閉）, so that 重開機後的長任務可以無人值守地恢復。【D-25】
83. As a 使用者, I want 完全退出後下次登入不會自動啟動任何背景工作，必須我手動開 App, so that Windows 更新重開機不會間接解除我的停止。
84. As a 使用者, I want 恢復時任何結果未知的外部操作都先查證，不能確認就送裁定, so that 不會重複部署、重複發訊或重複付款。
85. As a 使用者, I want 磁碟滿或資料庫不可寫時新派工停止並顯示可處理原因, so that 不會產生無法追溯的副作用。

### 安全與本機 API

86. As a 使用者, I want Runtime API 只監聽本機 loopback 且只有桌面 App 能管理, so that 網路上或瀏覽器中的其他頁面無法操作我的 Agent。【D-23】
87. As a 使用者, I want 桌面 App 自動完成連線驗證而不需要我複製 API 密鑰, so that 安全不增加操作負擔。【D-22】
88. As a 使用者, I want 每個 Agent 只能存取自己 Workspace／Run 範圍內的資料，不能批准裁定、擴大授權或取得管理能力, so that 一個失控的 Agent 影響範圍有限。
89. As a 使用者, I want 憑證永遠不出現在 URL、Prompt、日誌、transcript、成果或一般備份中, so that 分享成果或日誌不會外洩登入資料。

### 備份與觀測

90. As a 使用者, I want 資料庫與被引用的成果每日自動一致備份，並定期在隔離目錄實際還原驗證, so that 備份是真的可用的。【D-18】
91. As a 使用者, I want 還原後舊 Run 不會自動重做外部操作, so that 還原不會造成副作用重放。
92. As a 使用者, I want 事件時間線呈現每個 Task 的派工、交接、裁定、恢復與驗證事件, so that 我能回頭理解發生了什麼。
93. As a 使用者, I want 看到工作完成率、交接成功率、恢復耗時、裁定等待時間、教訓重犯率與每階段用量, so that 我能依真實數據調整門檻與方法。

## Implementation Decisions

### 交付範圍與順序

- **【D-02】** 本 spec 涵蓋第一版完整範圍。拆 ticket 時依 issue §15 的四個階段排序：
  1. 核心執行：控制資料庫、Runtime、Claude Adapter、多帳號並行、Task／Run、訊息、Artifact、Controller 對話、基本 Web UI、桌面生命週期（DR-01）、授權（DR-03）、本機 API（DR-04）、worktree（DR-02）。
  2. 交接恢復：額度觀測、保守派工、檢查點、主動交接、突然限流恢復、Outbox 與程序核對、崩潰恢復（DR-05）、監護（DR-06）、跨登入（DR-07）、平台辨識（DR-08）。
  3. 裁定與防護：裁定中心完整功能、版本校驗、決策權限表、教訓與 Guard、Context Builder、備份還原。
  4. 工程方法與驗證：Project Brief／Glossary／Project Map、Method Registry 與 Playbook Router、Stage／Gate、雙軸 Review、Verification Profile 與 Adapter、探索重現、Replay。
- 第一、二階段完成即構成可自用的 MVP；第一階段內的裁定中心只需「建立請求、顯示、單筆回答、喚醒工作」的最小版本。
- 每張 ticket 採垂直切片，穿過資料、Runtime 邏輯、API 與（必要時）UI，並以主要 seam 的測試驗收。
- **【D-03】** 重建期間保留現有 `skills/` 目錄與 README 不動；第一張 ticket 以 git tag 封存舊版 skill。移除舊 skill、改 repo 名稱與改寫 README 列為最後一張 `ready-for-human` ticket。
- **【D-01】** 產品暫名 Baton；程式中的產品名稱集中在單一常數與安裝設定，改名只影響這些位置與資料目錄名稱。

### 程序與模組架構

整體由四個可執行元件組成：

| 元件 | 職責 |
| --- | --- |
| 桌面 App（Tauri + React Web UI） | 唯一的人工操作入口；啟動或連回 Runtime；系統匣與完全退出 |
| Runtime（Rust + Tokio + Axum） | 唯一控制平面與唯一 SQLite writer；排程、程序管理、交接、恢復、API |
| 監護程序（獨立輕量 Rust 程式） | 同次登入期間監看 Runtime，依額度有限重啟；不寫控制資料庫 |
| MCP 工具 shim（獨立輕量 Rust 程式） | 由 Claude Code 以 stdio 啟動，把 Agent 的工具呼叫轉送到 Runtime 的 Agent API |

另有一個登入啟動 launcher（可與監護同一個執行檔的不同模式）。

Runtime 內部以 Rust workspace 分成以下深模組，介面小、內部吸收複雜度：

- **Domain 核心**：所有實體（Workspace、Goal、Task、Run、Session、Artifact、Decision Request／Decision、Lesson／Guard、Workflow／Stage）的狀態機與不變條件。純邏輯、無 I/O，輸入「目前狀態 + 命令 + 時間」，輸出「新狀態 + 事件 + Outbox 項目」。所有狀態轉換只能經過這裡。
- **Store**：SQLite 存取、寫入佇列（單一 writer task，所有寫入序列化為短交易）、migration、備份。對外只暴露「在一個交易中套用 Domain 結果」與唯讀查詢。
- **Scheduler**：依相依、優先序、能力、授權、帳號餘裕與執行槽選擇下一個派工；交接觸發；waiting_resource 喚醒。
- **Supervisor**：launch intent → 啟動 → 回執 → 就緒 → 停止；Job Object 管理；程序身分核對；reconciliation。
- **Outbox Dispatcher**：至少一次交付，所有路徑都先檢查停止意圖、恢復門檻與有效准入。
- **Recovery**：啟動時盤點、恢復事件／嘗試／退避／穩定觀察、監護日誌匯入。
- **Quota**：觀測器 Port 的實作與推估模型、門檻判斷。
- **Context Builder**：組裝 Task 契約、決策、教訓、檢查點與 context receipt。
- **Policy**：Workspace 授權、准入（R1-a）、E1 單次批准、決策權限表、permission-prompt 判定。
- **API**：Axum；桌面管理 API 與 Agent API 分開路由、分開憑證。
- **Runtime Adapter（Claude Code）**：見下節。

所有外部副作用都透過 Port 注入，作為測試 seam【D-04】：

```text
// 來自設計決策的介面形狀（非實作）
Port Clock            { now() }
Port ProcessControl   { spawn(spec) -> Receipt; inspect(identity) -> Alive|Dead|Unknown; stop(identity, grace) -> StopResult }
Port AgentAdapter     { launch(run_spec) -> Handle; events(handle) -> Stream<AdapterEvent>; resume_capability(session) -> Capability }
Port QuotaObserver    { observe(profile) -> QuotaSample { source, observed_at, used, reset_at?, confidence } }
Port WorkspaceFs      { create_worktree; remove_worktree; inspect_dirty; write_artifact_atomic }
Port PlatformInfo     { os_build; edition; native_arch; support_status }
```

### Claude Code Runtime Adapter

- **【D-06】** Controller 不是常駐程序。Runtime 在需要 Controller 判斷時（事件到達、合併 5 秒視窗後【D-31】）啟動一次 `claude -p`，以 stream-json 輸出，第一次建立原生 Session，之後以 resume 接續同一原生 Session；換帳號或原生 Session 不可用時依 issue §3.3 建立替代 Session，從持久化對話、決策與檢查點重建。同一 conversation 任何時刻最多一輪進行中（B1）。
- **【D-07】** 每個 Worker Run 是一次 `claude -p` 程序，Prompt 由 Context Builder 產生（Task 契約 + 適用決策／教訓 + 檢查點 + 工具使用規則）。Worker 透過 MCP 工具回報進度、checkpoint、成果與阻礙；程序結束只產生「Run 結束」事件，不推進 Task 完成。
- Adapter 解析 stream-json 事件：原生 Session 識別、工具使用、結果、錯誤與限流訊號；首次取得 native_session_id 後以事件回報並條件綁定。
- **【D-05】** Agent 工具介面：Runtime 為每個 Run 產生專屬 MCP 設定，指向 MCP shim；shim 從環境變數取得該 Run 的範圍憑證，透過 loopback 呼叫 Agent API。第一版工具集：
  - 共同：`get_task_contract`、`report_progress`、`save_checkpoint`、`register_artifact`、`report_blocked`、`request_decision`、`send_message`、`read_inbox`、`search_knowledge`。
  - Controller 專屬：`propose_task`、`update_task`、`add_dependency`、`dispatch_task`、`cancel_task`、`propose_decision_request`、`record_lesson_candidate`、`accept_artifact`（仍受驗證 Gate 約束）。
  - 權限：`permission_prompt`（見 D-08）。
- **【D-08】** Worker 以非互動權限模式執行，允許的工具清單由 Workspace 授權與 Task／Stage 的 allowed_actions 推導；未預先允許的操作經 permission-prompt 工具送到 Runtime：授權範圍內自動允許並記錄，範圍外建立裁定請求並讓該工具呼叫回傳「等待裁定」，Worker 須保存 checkpoint 並以 blocked 結束或改做其他步驟。具體 CLI 參數與最低版本為 **Validation Required**，Adapter 啟動前以版本探測決定能力，不支援時阻擋並顯示原因，不改為全面放行。
- **【D-09】** 設定版本固定：每個 Profile 的 CLAUDE_CONFIG_DIR 只承載原生登入與 Session 狀態；共同設定的每個採用版本存成不可變快照；Run 啟動時把該 Run 准入的快照寫入 Run 專屬唯讀目錄並以設定參數傳入，避免熱重載改變已准入 Run。是否能完全阻擋使用者層設定合併與熱重載為 **Validation Required**；無法維持時阻擋並顯示，不宣稱固定成功。
- **【D-30】** 平台預設不指定模型；Workspace 設定可依角色（Controller、Worker、Reviewer）覆寫。
- **【D-10】** 每個 Run 建立一個 Windows Job Object 並把 Claude Code 程序放入，確保子程序一併受控；程序身分 = PID + 程序建立時間 + 啟動指令雜湊，避免 PID 重用誤判。

### 資料模型與持久化

- 採用 issue §11.2 的完整邏輯資料表群組，不在此重複。補充：
- **【D-17】** 所有本機資料位於 `%LOCALAPPDATA%\Baton\`，分為控制資料庫、成果儲存、Profile 原生設定目錄、Run 專屬設定快照、備份、日誌、生命週期檔與監護日誌。拒絕位於網路共享或雲端同步資料夾的位置（啟動時檢查）。
- SQLite 設定採 issue §11.1 的 PRAGMA 初始值，啟動時讀回驗證。
- 所有寫入經 Store 的單一寫入佇列；Domain 核心產出的「狀態變更 + 事件 + Outbox」在同一短交易提交。
- 成果檔案先寫暫存、fsync、原子改名，再提交 manifest。
- **【D-26】** 決策、教訓、知識以 FTS5 + trigram tokenizer 建立可重建索引；啟動時檢查 FTS5 可用性，不可用時退回 LIKE 查詢並顯示降級。
- **【D-31】** heartbeat 15 秒、lease 60 秒；過期 lease 只觸發核對，不證明程序死亡。
- **【D-33】** 資料庫一律存 UTC，UI 以 Windows 系統時區顯示。

### 排程、額度與交接

- **【D-12】** 額度觀測器第一版有兩個來源：
  1. 「已觀測」：Adapter 從 CLI 串流與錯誤中解析到的限流訊號與重置時間。
  2. 「推估」：Runtime 依每個 Run 的結果訊息累計用量，以滾動五小時與七日視窗估算；使用者可在帳號頁為每個 Profile 設定方案上限作為分母。
  觀測不到的值存為未知，不填零。若後續驗證發現 CLI 提供直接的額度查詢，新增為第三個觀測器實作即可。**Validation Required**。
- **【D-13】** 門檻（以兩個視窗中較緊者計）：推估 ≥75% 不再派新 Run 給該帳號；≥85% 或收到已觀測限流警告即進入 draining 並對進行中的 Run 觸發交接；回到 <60% 或觀測到重置後才恢復可派。同一 Task 兩次換帳號至少間隔 10 分鐘（冷卻），除非是突然限流。所有數值為 Workspace 無關的全域設定，可在設定頁調整。
- **【D-14】** 預設每帳號最多 2 個並行 Run、全域最多 6 個；帳號數 ≥3 時保留 1 個全域執行槽給交接 successor。使用者可在「較高並行／保留交接容量」間切換預設組合。
- **【D-32】** 最多 8 個 account_profile。
- 交接協議依 issue §5.2 五步驟；Domain 核心以 handoff phase 狀態機表達，每一 phase 持久化後才執行下一步的外部副作用。
- 全部受限時進入 waiting_resource；有 reset_at 時於重置後 1 分鐘重新評估，未知時以 5 分鐘起、上限 60 分鐘的退避輪詢【D-31】。
- **【D-16】** 預算：實作失敗修正 3 次、驗證不完整重驗 2 次、基礎設施重試 5 次（指數退避，上限 10 分鐘），額度交接與恢復嘗試不計入以上任何一項。超過時 Task 進入 blocked 並建立裁定請求。

### 裁定與決策權限

- 裁定請求與 Decision 的欄位、expected_revision 校驗、批次逐項保存依 issue §6。
- **【D-15】** 決策權限表（第一版內建，Workspace 可收窄、不可放寬）：
  - Controller 可自主處理：在既有授權內拆分、合併、排序、重試 Task；選擇 Playbook 與方法版本；派研究或實驗 Worker；分類 review finding 為「修正」；在預算內重派；建立調查 Task；把候選教訓提交給使用者。
  - 必須送裁定：任何產品方向、使用者價值或範圍取捨；修改完成條件或驗收標準；擴大或超出 Workspace 授權的操作；外部寫入、部署、發訊、付費、刪除正式資料；駁回 review finding；跳過 Stage 或 Gate；結果未知外部操作的處置；接受設計偏離；預算用盡後的下一步；三次恢復失敗後的重試（此項只能人工從恢復介面發起）。
- 只有經桌面管理憑證提交者才是人工正式裁定；Agent 憑證只能提出與查詢。

### 生命週期、監護與跨登入

- 依 DR-01、05、06、07 全文實作。補充決策：
- **【D-11】** 完全退出：持久化停止意圖 → 停止新派工與交接 → 通知所有 Run 保存進度並停止（30 秒寬限）→ 以 Job Object 終止剩餘程序樹並確認（10 秒）→ 無法確認者記錄停止異常。數值為 **Validation Required** 的初始值。
- **【D-24】** 監護與 Runtime 銜接：監護在自動啟動 Runtime 前，把「事件識別、輪次、嘗試序號、時間、原因」以單行紀錄 append 到監護日誌並 fsync；Runtime 啟動後第一件事是匯入尚未匯入的紀錄到控制資料庫（以嘗試識別去重），再開始任何恢復副作用。監護從日誌與生命週期檔即可算出已用次數與退避期限，不需讀取 SQLite。
- **【D-25】** 停止意圖同時存在控制資料庫與一個原子寫入的生命週期檔；兩者不一致時採較保守者（視為停止）。登入自啟動以目前使用者的 HKCU Run 機碼啟動 launcher；launcher 先檢查生命週期檔中的停止抑制與維護抑制，受抑制則直接結束。
- 單實例：Runtime 以具名 mutex 取得單實例鎖；App、launcher、監護在啟動 Runtime 前都先嘗試連線既有 Runtime。
- **【D-19】** 安裝與更新：Tauri NSIS 單使用者安裝；更新檢查啟動時與每日一次，只通知；使用者同意後進入維護抑制（寫入生命週期檔）、執行與完全退出相同的停止流程、備份、套用、再依 DR-01 規則重新開啟。解除安裝前同樣執行停止流程並移除 HKCU Run 機碼，資料目錄預設保留。

### 本機 API 與安全

- **【D-22】** 憑證：256-bit 隨機 bearer token，控制資料庫只存雜湊與關聯（呼叫身分、角色、Workspace、Run、世代、到期）。桌面管理憑證由 Runtime 每次啟動時輪替，寫入以 DPAPI（目前使用者範圍）保護的檔案，App 讀取後連線；Agent 憑證於 Run 准入時簽發，只經環境變數交給 MCP shim，Run 結束或被接替時失效。
- **【D-23】** 網頁存取防護：所有端點（含串流）要求 Authorization 標頭與固定自訂標頭；檢查 Host 為 loopback 位址與既定埠、Origin 若存在必須為 Tauri 的 App origin；不回應任何 CORS 預檢；同時綁定 IPv4 與 IPv6 loopback，不監聽其他位址。
- 秘密只以參照存在資料庫；日誌與 transcript 匯入時以已知秘密樣式遮蔽。

### 操作介面

- **【D-21】** 版面：左側為 Workspace 清單與全域入口（帳號、設定、平台狀態）；主區分頁為「對話」「工作」「裁定」「成果與驗證」「知識」「時間線」；右側為選取 Task／Run／裁定的詳細面板。頂部狀態列固定顯示：Runtime 狀態、監護狀態、可用帳號數、待裁定數、恢復事件狀態。
- 工作分頁以 Task 狀態分欄；每張卡顯示執行者、帳號、等待原因、最後有效進度、交接狀態、清理狀態。
- 第四階段再加入專案理解（Brief／Glossary／Decision Map）、流程（Playbook／Stage／Gate）、驗證旅程與證據視圖。
- UI 與 Runtime 以 HTTP + Server-Sent Events 溝通；UI 不持有任何資料庫交易，所有狀態以 Runtime API 為準。

### 首次啟動與平台

- **【D-20】** 首次啟動精靈步驟：平台辨識（DR-08）→ Claude Code 是否在 PATH、版本 → Git for Windows → 建立第一個 Profile 並引導在 App 開啟的終端機內以該 Profile 的 CLAUDE_CONFIG_DIR 執行原生登入 → 核對實際認證帳號 → 匯入共同設定 → 告知背景行為 → 建立第一個 Workspace（授權範本確認）。缺項只引導，不代為下載或安裝。最低支援的 Claude Code 版本於 Adapter 驗證時固定。
- 平台辨識依 DR-08：Windows 11 x64 Home／Pro，25H2、26H2 為正式支援；ARM 上 x64 模擬、Windows 10、其他版本別顯示為非正式支援但不阻擋使用。

### 工程方法與驗證（第四階段）

- 方法與 Playbook 以版本化內容保存於控制資料庫（結構化索引）與成果儲存（原文）；第一版內建 issue §9.1 的七個 Playbook。
- **【D-29】** Verification Adapter 第一版只實作：CLI／API（執行命令與 HTTP 請求、程式斷言）與 web（以 Playwright 為基礎的操作與斷言，tester-army/e2e 作為候選引擎、整合前驗證授權與契約）。mobile 延後。
- 驗證 Run 與一般 Run 共用帳號額度與交接機制。

### 知識與教訓

- **【D-28】** 教訓流程中「候選 → 已驗證」與「啟用 Guard」都需要使用者在知識分頁確認；Controller 只能提交候選與證據。
- **【D-27】** Worker 間訊息存入資料庫並經 `read_inbox` 拉取；Context Builder 在每次 Run 啟動時附上未讀摘要；Runtime 不在 Worker 執行中途注入訊息。訊息不能改變正式工作歸屬與範圍。

### 備份

- **【D-18】** 使用 SQLite Online Backup API；觸發時機為每日一次、migration 前、以及重要裁定或 Stage 完成後（同一小時內合併為一次）。保留最近 7 份每日備份與 4 份每週備份；每份備份連同被引用的不可變成果 manifest 一起建立。每週一次自動在隔離暫存目錄還原並核對資料庫完整性、成果雜湊與關聯，結果顯示於設定頁。異地副本：使用者可另外指定一個額外備份目錄（非必要）。

## Testing Decisions

### 好測試的標準

- 只測外部行為：透過 seam 送入命令與事件，斷言 API 回應、可觀察狀態、事件與 Port 被呼叫的方式（例如「沒有第二次 spawn」）。不斷言資料表欄位、內部函式或呼叫順序細節。
- 每個 issue 驗收案例（§15、§15.1～15.13）至少對應一個情境測試，測試名稱使用 issue 的領域詞彙。
- 故障注入是一級公民：Port 的 fake 可以在任何邊界「崩潰」（例如 spawn 已執行但回執未寫回），測試重建 Runtime 後斷言恢復行為。
- 時間一律由 fake Clock 控制，不使用真實 sleep。

### 測試 seam【D-04】

1. **主要 seam：Runtime harness。** 在測試中以程序內方式啟動完整 Runtime（真實 Domain 核心、真實 Store 與暫存目錄中的真實 SQLite、真實 Scheduler／Supervisor／Outbox），只替換 Port：fake Clock、scripted fake AgentAdapter（可模擬 Worker 回報、checkpoint、限流、崩潰、原生 Session 遺失）、fake QuotaObserver、fake ProcessControl（可模擬存活、死亡、未知、停止失敗）、暫存目錄的 WorkspaceFs、fake PlatformInfo。測試透過真實的 Axum API（桌面與 Agent 兩組憑證）操作。「重啟 Runtime」= 丟棄 Runtime 實例、保留 SQLite 與檔案、以同一組 fake 重新建立。絕大多數測試（交接、恢復、授權競態、裁定、Outbox、清理、生命週期）都在這一層。
2. **Adapter 契約測試。** 同一份契約測試套件同時跑在 fake AgentAdapter 與真實 Claude Code Adapter 上，確保 fake 的行為與真實一致。真實版只在 Windows 原生、已登入測試帳號的環境執行（手動觸發或夜間），對應所有 **Validation Required** 項目。
3. **少量 UI E2E。** 以 Playwright 對內嵌 Web UI（連到使用 fake Port 的 Runtime）驗證關鍵旅程：首次精靈、派工到完成、裁定中心批次處理、完全退出進度、恢復事件顯示。
4. Domain 核心的狀態機另以性質測試（property-based）檢查不變條件，例如「同一 Task 最多一個有效寫入所有權」「過期 generation 的回報不改變狀態」「恢復嘗試數永不超過 3」。這是主要 seam 的補充，不取代情境測試。
5. Windows 平台驗收（DR-08 的四種組合）以手動執行的驗收清單 + 真實 Adapter 契約測試完成，結果記錄 OS build、App、Claude Code 與相依版本。

### 受測模組

- 透過主要 seam：Scheduler、Supervisor、Outbox、Recovery、Quota、Policy、Context Builder、裁定、worktree 清理、生命週期、API 身分驗證。
- 透過 Adapter 契約：Claude Code Adapter、MCP shim。
- 透過性質測試：Domain 核心。
- 透過 UI E2E：Web UI 關鍵旅程。

### Prior art

- 本 repo 目前只有 Markdown skill，沒有任何可沿用的測試程式碼。
- 概念上的前例是現有 issue-orchestrator skill 的 write-ahead 狀態轉換、controller epoch「每次從磁碟重建」、verifier 與 worker 分離：Runtime 恢復測試應延續「只信任持久化狀態」的原則。
- 測試寫法參考 Rust 社群慣例：`tokio::test` 搭配暫存目錄、Axum 的程序內 router 測試、`proptest` 性質測試。

## Out of Scope

- macOS、Linux、WSL 執行環境；Windows 10、Server、ARM64 原生與 ARM 上 x64 模擬的正式支援。
- 遠端 Runner、多機控制平面、一般瀏覽器管理入口、遠端裝置配對、多人管理。
- 系統服務模式；登入前、登出期間、睡眠或關機時持續工作。
- OS 層級沙箱，或對可信專案與依賴程式內部副作用的全面攔截保證。
- Claude Code 以外的 Agent Runtime（核心資料模型保留擴充空間，但不實作）。
- mobile 驗證 Adapter【D-29】；向量或圖搜尋。
- 自動刪除 Git 分支、Git 歷史或正式 Artifact；回滾已發生的外部副作用。
- 存活 Run 直接接管、自適應退避、進階診斷、監護自我修復、無回應偵測與受控重啟。
- 指定時段啟動、延遲啟動、一次性略過自啟動；全域批次續跑設定、「做完後退出」。
- 手動保留 worktree 現場、保留期限設定、空間管理面板；設定分享、進階同步與差異面板。
- 代為下載安裝 Claude Code 或 Git【D-20】。
- 異地備份的自動上傳【D-18】。

## Further Notes

- **Validation Required 清單**（第二階段結束前須以 Adapter 契約測試或 Windows 驗收記錄結論）：CLI 非互動權限參數與 permission-prompt 工具、設定快照能否阻擋熱重載與使用者層合併、原生 Session resume 行為、stream-json 中的限流訊號格式、Job Object 程序樹終止、DPAPI 憑證交付、HKCU Run 登入啟動、監護日誌與資料庫銜接、四種 Windows 組合相容性、30 秒／2 分鐘／10 分鐘／40 秒等時間參數。驗證結果與本 spec 的 Claude 決策衝突時，修改對應 D-NN 並記錄原因，不靜默改變 issue 中已確認的 DR 政策。
- 建議在第一階段完成、開始自用後，優先依實際感受回頭檢視這些最可能需要調整的決策：D-13（交接門檻）、D-14（並行容量）、D-15（決策權限表）、D-21（UI 版面）、D-16（預算）。
- issue #3 的參考專案（Overstory、Warren、multi-claude、Agent Inbox、Temporal 範例、Mulch、Graphiti、Agent Studio）與三個方法來源（mattpocock/skills、pstack-claude、tester-army/e2e，已固定 commit）只作為設計參考，不作為相依套件。
- repo 目前沒有 GLOSSARY.md 與 ADR；本 spec 使用 issue #3 的詞彙（Workspace、Goal、Task、Run、Session、Artifact、Decision Request、Decision、Lesson、Guard、Controller、Worker、Runtime、Workflow、Stage、Gate、Playbook、Verification Profile、Evidence Bundle、handoff、checkpoint、draining）。第一張 ticket 建議以 `/domain-modeling` 把這些詞彙落成 GLOSSARY.md，並把 DR-01～08、DR-20261022-01～03 與本 spec 中高改動成本的 Claude 決策（D-04、D-05）記為 ADR。
