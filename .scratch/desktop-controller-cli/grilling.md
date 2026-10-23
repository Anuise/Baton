# 對抗式問答：桌面頁簽化與原生 CLI Controller

Status: resolved（已據此寫成 [spec.md](spec.md)；決策仍為 AI 暫定）

本檔是 AI 代替使用者做的對抗式問答紀錄。使用者原始意見：

1. 桌面版 UI/UX 太醜，要用頁簽把每個功能切開。
2. 對話方式太爛，希望選定一個 work 當 Controller，直接開啟 CLI 使用原生 Claude Code。
3. 運作方式參考 `.scratch/platform-rebuild-v1` 用 prompt 讓 Claude 工作的方式。

每題格式：問題 → AI 的回答（＝暫定決策）→ 重要度 → 被否決的替代方案。重要度定義沿用 `platform-rebuild-v1/ai-decisions.md`（高：做錯會白費額度／資料／大量工作；中：影響工作方式；低：預設值）。**標 ⚠ 的題目請優先看。**

相關文件：[ADR-0014](../../docs/adr/0014-native-cli-controller.md)、[GLOSSARY.md](../../GLOSSARY.md)（新增「Controller CLI」「Controller 帳號」，修改「Controller」「桌面 App」）。

---

## 現況查證（AI 自行查的事實，不是決策）

- `web/src/App.tsx`：主區把精靈、帳號、排程、跨登入、更新、共同設定、備份、恢復、平台狀態**全部同時往下疊**，再把選中 Workspace 的某個畫面接在後面。Workspace 的 12 個功能在左側清單以按鈕切換。spec D-21 原本就規定「主區分頁」，實作沒有做到。
- 現行對話（票 16、ADR-0009）：使用者在 `ConversationPage` 打字 → Runtime 合併 5 秒事件 → 每輪啟動一次 `claude -p` 並 resume → 前端每 3 秒輪詢。使用者看不到 Controller 的工具呼叫過程，也不能用 Claude Code 原生的指令、快捷鍵、skills。
- Controller 已有完整 MCP 工具（票 17、54）：`propose_task`、`dispatch_task`、`cancel_task`、`send_message`、`read_inbox`、`search_knowledge`、`accept_artifact` 等，走 MCP shim + Agent 憑證（ADR-0013）。
- Runtime 已有「開一個終端機視窗、以某 Profile 的 `CLAUDE_CONFIG_DIR` 執行原生登入」的 port（`baton-ports/src/claude_env.rs` 的 `open_login_terminal`）。
- Account Profile 的設定目錄一律由平台建在 `<資料目錄>\profiles\<id>`，**不能指向既有的 `~/.claude-Work-n`**。
- `.scratch` 的實際運作：使用者在互動式 Claude Code（Work-0）當主控 → 主控讀 `execution.md`／`ai-decisions.md` → 以 `dispatch.sh` 在背景用 Work-n 跑 `claude -p` → 背景程序結束時 Claude Code 原生通知主控 → 主控派獨立驗證 run → 不過就修正（最多 3 次）→ 遇到問題自己決定並寫進 `ai-decisions.md`（附重要度，使用者事後審）。

---

## 第 1 輪（沒有前置條件的題目）

### Q1 ⚠ Controller 改成「原生互動式 Claude Code」，取代 `claude -p` 每輪喚醒？
- **回答**：是。Controller 改為使用者在終端機裡直接操作的互動式 `claude`（不是 `-p`）。平台不再替使用者轉述對話，也不再自動喚醒 Controller 輪次。
- 重要度：高（推翻 ADR-0009 的 A1／B1，見 ADR-0014）
- 否決：保留 `claude -p` 輪次、只改善對話頁 UI——根本問題（看不到過程、不能用原生功能、要等輪詢）不會消失。

### Q2 ⚠ CLI 開在「外部終端機視窗」還是「桌面 App 裡的內嵌終端機頁簽」？
- **回答**：外部終端機視窗。有 Windows Terminal（`wt.exe`）就用它開新視窗，沒有就用系統預設主控台。桌面 App 的 Controller 頁簽只負責「選帳號、開啟、顯示狀態、結束」。
- 理由：原生體驗 100%（快捷鍵、複製貼上、滑鼠、IME 中文輸入都是終端機自己處理）；Runtime 已有開終端機的 port，可直接延伸；內嵌需要 ConPTY + xterm.js，Claude Code 的 TUI 在內嵌終端常有重繪與輸入法問題，工作量大一個量級。
- 重要度：高
- 否決：內嵌 xterm.js 頁簽——畫面統一，但成本高、原生體驗打折，且違反「直接用原生 Claude Code 就好」。之後要內嵌可另開票，不影響本次其他決策。

### Q3 ⚠「選定一個 work」的 work 指什麼？
- **回答**：指平台的 **Account Profile**。另外新增「以既有目錄建立 Profile」：讓使用者把 `~/.claude-Work-n` 直接登記成 Profile，不必每個帳號重新登入一次。
- 理由：平台只認 Profile；但使用者手上已有 5 個登入好的 Work-n，重登成本高。
- 重要度：中
- 否決：(a) 只能用平台新建的 Profile——每個帳號要重登。(b) Controller 直接吃 `.claude-Work-n` 路徑、不經 Profile——額度觀測、派工排除都對不上。
- 風險：使用者若同時在平台外拿同一個 Work-n 跑 `.scratch` 的 `claude -p`，平台的並行計數看不到；額度觀測仍是真實用量。

### Q4 被選為 Controller 的帳號，開啟期間還能派 Worker 嗎？
- **回答**：不能。Controller CLI 開啟期間，該 Profile 從 Worker 派工候選中排除（等同 `.scratch` 的「Work-0 不跑 `claude -p`」規則）；CLI 結束後自動恢復。
- 重要度：中
- 否決：照常參與派工——Controller 自己的互動額度會被 Worker 吃掉，正是 Work-0 規則要避免的事。

### Q5 舊的對話頁與自動喚醒怎麼處理？
- **回答**：移除。拿掉「對話」頁簽與 Runtime 的 Controller 自動喚醒輪次（事件合併、`claude -p` resume 那條路徑）。既有 conversation 資料表保留、不遷移、不再寫入。
- 重要度：中
- 否決：兩種模式並存——要維護兩套 Controller 生命週期，且使用者已明確說舊方式不好用。

### Q6 頁簽怎麼切？
- **回答**：兩層。
  - **第一層（頂部）**：`Workspace`｜`帳號`｜`設定`。
  - **Workspace 第二層**（左側仍是 Workspace 清單，選中後主區顯示這組頁簽）：
    1. **Controller**（新）：選帳號、開啟／結束 CLI、狀態、該帳號額度
    2. **工作**（TaskBoard）
    3. **裁定**（DecisionCenter，徽章顯示待處理數）
    4. **成果**（Artifacts）
    5. **審查與驗證**（Review／Verification／Exploration，以第三層小頁簽切換）
    6. **知識**（Knowledge＋Project Understanding）
    7. **流程**（Workflows）
    8. **時間線**
    9. **授權**（Authorization＋Model Overrides）
  - **帳號**：Accounts＋Scheduling＋Shared Settings。
  - **設定**：跨登入啟動、更新、備份、恢復、平台狀態。
  - 首次啟動精靈（SetupWizard）在沒有任何可用 Profile 時以全畫面顯示，不放進頁簽。狀態列、更新通知、完全退出覆蓋層維持在頁簽之外。
- 重要度：低（D-21 本來就要分頁，改動便宜）
- 否決：一層 15 個頁簽——太擠；保留左側按鈕清單只把全域設定收起來——沒解決「每個功能切開」。

### Q7 除了頁簽，要不要順便改視覺風格？
- **回答**：本次只做資訊架構（頁簽、一次只顯示一個功能、間距與標題層級一致），不換配色、不導入 UI 元件庫。
- 重要度：低
- 否決：全面重設計——範圍無法收斂；使用者也還沒說清楚「醜」的具體點。做完頁簽再看。

---

## 第 2 輪（依賴 Q1～Q3）

### Q8 ⚠ 原生 CLI 怎麼拿到平台的 Controller 工具？
- **回答**：開啟時 Runtime 建立一個 **Controller Run**、簽發 Controller 範圍的 Agent 憑證，啟動參數帶：
  - `CLAUDE_CONFIG_DIR` = 選定 Profile；先清掉繼承的 `CLAUDE*`／`ANTHROPIC*`／`AI_AGENT`／`BAGGAGE`（沿用 AI-02）。
  - `--mcp-config` 指向 MCP shim（憑證只走環境變數，沿用 ADR-0013）＋共同設定的 MCP 伺服器；設定快照與 Worker 相同機制。
  - `--append-system-prompt-file` = Controller 操作手冊（Q10）。
  - 工作目錄 = Workspace 目錄，所以 Workspace 自己的 `CLAUDE.md` 會被原生載入。
- 重要度：高
- 否決：讓 Controller 用 curl 打 HTTP API——ADR-0013 已否決（憑證暴露）。

### Q9 ⚠ Worker 做完時，原生 CLI 怎麼知道？
- **回答**：比照 `.scratch` 的「背景程序結束就通知」：提供一個等待指令（由 shim 執行檔加一個子命令，例如 `baton-mcp-shim wait`），Controller 以 Claude Code 的**背景 Bash** 執行它；有新事件（Run 結束、受阻、裁定被回答、訊息）時印出事件摘要並結束，Claude Code 原生通知 Controller。憑證由 CLI 環境變數繼承，不出現在命令列。操作手冊規定「派工後要掛著等待」。
- 重要度：高
- 否決：(a) MCP 工具長輪詢 `wait_events`——會卡住整個回合，使用者插話體驗差，還有工具逾時。(b) Runtime 把文字打進終端機——外部視窗做不到，內嵌也很 hacky。(c) 不通知，要使用者自己問——失去自主連續工作的能力。

### Q10 ⚠「參考 `.scratch` 用 prompt 讓 Claude 工作的方式」具體是什麼？
- **回答**：把 `execution.md` 的流程**平台化**成內建的「Controller 操作手冊」（隨 App 發佈、有版本），內容：
  1. 讀 Task 面板找 frontier（相依都完成的 Task），依序處理。
  2. 用 `dispatch_task` 派實作 Worker；帳號挑選、額度門檻（5h／7d 剩餘 ≥ 3%）、每帳號並行 < 3、環境清理、watchdog、stream-json 紀錄**都由 Runtime 的排程設定負責，不寫在 prompt 裡**。
  3. 實作完成後派**另一個** Worker 做唯讀驗證；不過就派修正，最多 3 次，超過標 blocked 跳下一個。
  4. 派工後以背景等待指令掛著（Q9）。
  5. 遇到不明確處先自己決定、繼續做，記錄到 AI 決策紀錄（Q11）；只有決策權限表（D-15）規定必須問的才開 Decision Request。
  - Workspace 可以用自己的 `CLAUDE.md` 補充或收窄，不能放寬授權。
- 重要度：高
- 否決：直接把 `.scratch/platform-rebuild-v1/execution.md` 原文塞給 Controller——它寫死 `dispatch.sh`、Work-0、worktree 路徑，跟平台的派工機制衝突。

### Q11 ⚠「自己決定、記下來、使用者事後審」要放平台資料庫還是檔案？
- **回答**：檔案。Controller 與 Worker 把 AI 決策追加到 Workspace 內的 `ai-decisions.md`（路徑由操作手冊指定，格式與現行完全相同：編號、重要度、審閱行）。這樣既有的 `review-ai-decisions` skill 可以直接用。
- 重要度：中
- 否決：新增平台實體「AI 決策紀錄」＋審閱頁——功能更完整，但要新資料模型與 UI，現在沒有證據需要。之後覺得不夠再升級。
- 注意：Workspace 不一定是 Git repo，檔案不會自動版控。

### Q12 實作 Worker 的指示（`.scratch` 的 impl／verify／fix 範本）怎麼對應？
- **回答**：不另做範本系統。Controller 用 Task 的六項最小契約寫清楚目標、範圍、完成條件；Runtime 在 Worker 啟動時附加固定的 Worker 通則（TDD、commit 到該 Task 的 worktree 分支、可開最多 3 個 subagent、AI 決策寫法）。驗證與修正只是不同契約的 Task。
- 重要度：中
- 否決：把三個 `.template.md` 搬進平台當可編輯範本——多一套設定 UI，與六項契約重疊。

### Q13 Controller CLI 在平台眼中是什麼？
- **回答**：一個 **Controller Run**（互動式），綁 Workspace、Profile、程序識別與 Agent 憑證；CLI 程序結束＝Run 結束＝憑證失效。同一 Workspace 同時最多一個 Controller CLI；同一 Profile 可以同時當多個 Workspace 的 Controller（反正都已排除在 Worker 之外）。
- 重要度：中
- 否決：不建 Run、只開視窗——憑證、時間線、恢復都沒有依據。

---

## 第 3 輪（依賴 Q8、Q13）

### Q14 Controller 帳號額度用完怎麼辦？
- **回答**：不做自動交接。Controller 頁簽顯示該帳號的 5h／7d 額度；接近上限時提示。使用者可按「換帳號重開」：結束目前 CLI、以新 Profile 開新 Session。跨帳號不 resume（原生 Session 存在各自的設定目錄），新 Session 靠平台狀態（Task、裁定、成果、ai-decisions.md）接續。同帳號重開時以 `--continue` 接回上次的原生 Session。
- 重要度：中
- 否決：沿用票 30 的 Controller 自動交接——那套是為 `claude -p` 輪次設計的，互動式 CLI 不能被平台中途換掉。

### Q15 Controller CLI 的權限模式？
- **回答**：Claude Code 預設（互動詢問），不用 `bypassPermissions`。使用者在場，自己可用 Shift+Tab 切換。派出去的 Worker 仍受 Workspace 授權與 `permission_prompt` 約束，與 Controller 的權限模式無關。
- 重要度：中
- 否決：bypass——Controller 在 Workspace 目錄有寫入能力，沒必要預設放開。

### Q16 完全退出、Runtime 當掉、App 關閉時，CLI 視窗怎麼辦？
- **回答**：
  - **關閉 App 視窗（Runtime 仍在）**：CLI 不受影響。
  - **完全退出**：Runtime 結束 Controller CLI 程序（視窗關閉），退出進度畫面列出這一項；之後可以用同帳號 `--continue` 接回。
  - **Runtime 當掉**：CLI 留著，但平台工具會失敗；Runtime 恢復後該 Controller Run 標為 interrupted、舊憑證失效，Controller 頁簽提示「重新開啟」。
- 重要度：中
- 否決：完全退出時保留 CLI——Runtime 已停，CLI 只剩殘廢的工具，還會讓人以為平台還在跑。

### Q17 怎麼測？
- **回答**：Runtime 側用 harness（D-04）＋假的終端機 port 驗證：開啟會建 Controller Run、帶對的環境與參數、帳號被排除在派工外、程序結束會收尾、等待指令在事件到達時結束。UI 側沿用票 53 的 fake ports E2E 驗證頁簽切換。真實開窗與真實 `claude` 互動列為手動驗收（Windows 驗收清單補一條）。
- 重要度：低

---

## 待使用者確認的清單（依重要度）

| # | 題目 | AI 回答一句話 | 重要度 |
| --- | --- | --- | --- |
| Q1 | Controller 形態 | 原生互動式 `claude`，移除 `claude -p` 輪次 | 高 |
| Q2 | CLI 開在哪 | 外部終端機視窗，不內嵌 | 高 |
| Q8 | 工具怎麼接 | MCP shim＋Controller 憑證＋操作手冊 system prompt | 高 |
| Q9 | 怎麼收到 Worker 結果 | 背景 Bash 執行等待指令，事件到就結束→原生通知 | 高 |
| Q10 | 運作方式 | `execution.md` 平台化為內建操作手冊；帳號／額度規則交給 Runtime | 高 |
| Q3 | work＝？ | Account Profile；新增「以既有 `.claude-Work-n` 建 Profile」 | 中 |
| Q4 | Controller 帳號派工 | 開啟期間排除 | 中 |
| Q5 | 舊對話頁 | 移除 | 中 |
| Q11 | AI 決策紀錄 | Workspace 內的 `ai-decisions.md` 檔案 | 中 |
| Q12 | Worker 指示 | 六項契約＋Runtime 固定通則，不做範本 | 中 |
| Q13 | Controller CLI 的身分 | 一個互動式 Controller Run，每 Workspace 最多一個 | 中 |
| Q14 | 額度用完 | 手動換帳號重開，不自動交接 | 中 |
| Q15 | 權限模式 | 預設互動詢問 | 中 |
| Q16 | 退出／當掉 | 完全退出時關掉 CLI；當掉後提示重開 | 中 |
| Q6 | 頁簽結構 | 兩層：Workspace／帳號／設定，Workspace 下 9 個頁簽 | 低 |
| Q7 | 視覺 | 只做資訊架構，不換風格 | 低 |
| Q17 | 測試 | harness＋fake port；真實開窗手動驗收 | 低 |

## 刻意不做（範圍外）

- 內嵌終端機、Controller 自動交接、AI 決策的平台化審閱頁、UI 配色／元件庫重設計、可編輯的 Worker 指示範本。

---

## to-spec 期間新增的決策

### Q18 Runtime 怎麼追蹤開在外部終端機裡的 `claude`？
- **回答**：終端機裡執行的是 MCP shim 執行檔的「Controller 宿主」子命令，由它啟動 `claude`（繼承主控台），並在啟動與結束時向 Runtime 回報；宿主被強制關閉時，Runtime 以既有的程序存活查驗發現。
- 理由：從 Windows Terminal 開出的程序不是 Runtime 的子程序，Runtime 拿不到可靠的結束通知。
- 重要度：中
- 否決：只靠定期查驗程序存活——結束要等下一次查驗才被發現，帳號排除會延遲解除。

### Q19 AI 決策紀錄放在 Workspace 的哪裡？
- **回答**：`<Workspace>/.baton/ai-decisions.md`，不存在時開啟 Controller 前以標頭範本建立。
- 重要度：低
- 否決：Workspace 根目錄——會混進使用者自己的檔案。

### Q20 App 要不要記住上次所在的頁簽？
- **回答**：要，存在本機瀏覽器儲存（每個檢視者的偏好），讀不到就預設 Workspace／Controller。
- 重要度：低
