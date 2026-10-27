# Windows 平台手動驗收清單

這份清單需要真實的 Windows、已安裝並登入的 Claude Code 與桌面 App，因此由人執行。每次執行都記錄 OS build、App 與 Claude Code 版本、執行日期與結果（通過／失敗／發現事項）。自動化測試（Runtime harness、UI E2E，見 [development.md](development.md)）以 fake Port 涵蓋同樣的流程；這份清單只驗證真實環境才看得到的部分：真實的終端機視窗、原生 `claude`、程序樹與帳號目錄。

前置條件：一個已確認授權的 Workspace，以及至少兩個可派工的 Account Profile。C2 需要只有一個可派工 Profile 的環境，E5 的封存無法復原，兩者都建議用測試用的資料目錄執行。

## 執行紀錄

| 日期 | OS build | App 版本 | Claude Code 版本 | 執行者 | 結果 |
|---|---|---|---|---|---|
| | | | | | |

## A. Controller CLI 開窗與互動

A1～A7 原為票 03 的驗收項目。

| 編號 | 步驟 | 預期結果 |
|---|---|---|
| A1 | 在 Workspace 的 Controller 頁簽選一個已登入的 Profile，按「開啟」。 | 有 Windows Terminal（Windows 11 的預設終端機）時在其中開新視窗；沒有或預設終端機設為主控台主機時用系統主控台。頁簽狀態變成執行中。 |
| A2 | 看視窗標題。 | 標題含「Baton Controller」。 |
| A3 | 在終端機執行 `cd`（或看提示字元）。 | 工作目錄是該 Workspace。 |
| A4 | 在 `claude` 中輸入 `/status`。 | `claude` 以互動模式執行（命令列沒有 `-p`），顯示的帳號是選定 Profile 的帳號。 |
| A5 | 輸入 `/mcp`。 | 看得到 baton 伺服器，Controller 工具可用。 |
| A6 | 在終端機輸入 `/exit`；另開一次後改為直接關閉視窗。 | 兩種方式下，頁簽都在數秒內變成「已結束」。 |
| A7 | 再開一次，在 App 按「結束」。 | 宿主與 `claude` 整棵程序樹結束，視窗關閉；頁簽變成「已結束」。若 Windows Terminal 保留分頁並顯示程序結束碼，記錄為發現事項（取決於設定檔的 `closeOnExit`）。 |
| A8 | 把 Windows 的預設終端機應用程式改成「Windows 主控台主機」，再按「開啟」；驗收後改回原設定。 | 以系統主控台開窗，標題含「Baton Controller」，`/status` 顯示同一個帳號；頁簽狀態變成執行中，按「結束」後視窗關閉、頁簽變成「已結束」。 |
| A9 | 開啟前在 Workspace 放一個 `CLAUDE.md`，寫一條容易辨認的指示（例如「每次回答結尾加上『驗收標記』」）；開啟後用一般文字和 Controller 對話。 | 回覆在終端機中即時顯示，結尾帶「驗收標記」，表示 Workspace 的 `CLAUDE.md` 已被原生載入。 |
| A10 | 輸入 `/`，再執行一個內建指令（例如 `/help`）。 | 出現 Claude Code 原生的指令選單，指令照常執行。 |
| A11 | 問 Controller：「依照你的操作手冊，派工之後要怎麼等待 Worker 的結果？」 | 回答提到以 Bash 背景執行 `"$BATON_MCP_SHIM" wait`，以及結束碼 0、3、4 的處理方式；Controller 頁簽的「操作手冊版本」與手冊標題「Baton Controller 操作手冊（版本 1）」一致。 |
| A12 | 查看 Workspace 目錄。 | 有 `.baton\ai-decisions.md`，標題是「# AI 決策紀錄」；開啟前已存在的檔案內容不變。 |
| A13 | 請 Controller 建立一個 Task（例如「建立一個研究 Task：整理 README 的章節」）。 | Controller 呼叫 `propose_task`，不需要逐次確認；App 的「工作」頁簽在「待處理」欄看得到這個 Task。 |
| A14 | 請 Controller 在 Workspace 建立一個文字檔。 | Claude Code 依預設權限模式先詢問是否允許寫入檔案；Baton 自己的工具（A13）則不詢問。 |

## B. 事件等待

| 編號 | 步驟 | 預期結果 |
|---|---|---|
| B1 | 請 Controller 建立一個契約完整的小 Task 並派工，再依操作手冊掛上等待。 | Controller 呼叫 `dispatch_task`，以 Bash 的背景執行跑 `"$BATON_MCP_SHIM" wait`；等待期間仍可繼續和 Controller 對話。「工作」頁簽的 Task 變成執行中。 |
| B2 | 不要輸入任何東西，等 Worker Run 結束。 | 背景指令自行結束，Claude Code 原生通知 Controller。輸出以「Baton 事件（N 件）：」開頭，含「- [Run 結束] Task「<標題>」（<Task id>）的 Worker Run <Run id> …」，最後一行是「以上事件已確認，不會再回報；…」。Controller 依摘要處理後重新掛上等待。 |
| B3 | B2 之後請 Controller 在前景執行 `"$BATON_MCP_SHIM" wait --timeout 5; echo "結束碼 $?"`。 | 沒有再收到 B2 的事件；約 5 秒後印出「等待逾時：5 秒內沒有新事件；需要時再執行 wait。」與「結束碼 4」。 |
| B4 | 確認沒有掛著的 wait，從「工作」頁簽派一個小 Task 並等它結束；再請 Controller 執行 `"$BATON_MCP_SHIM" wait; echo "結束碼 $?"`。 | wait 立刻結束，印出這個 Run 結束的事件與「結束碼 0」。 |
| B5 | Controller CLI 開著、背景 wait 掛著時，在工作管理員結束 `baton-runtime.exe`（不要結束 `baton-guardian.exe`），等監護重新啟動 Runtime。 | 掛著的 wait 以結束碼 1 結束（「等待事件失敗（無法連線到 Runtime）」）。Runtime 重新啟動後，Controller 頁簽顯示「Runtime 重新啟動過，這個 Controller CLI 的平台工具已失效。…」，狀態仍是執行中。 |
| B6 | 接著請 Controller 執行 `"$BATON_MCP_SHIM" wait; echo "結束碼 $?"`。 | 印出「baton-mcp-shim wait：Controller 憑證已失效（Controller CLI 已結束），不要再執行 wait」與「結束碼 3」；Controller 依操作手冊不再掛等待，請使用者從 App 重新開啟。按「結束」後頁簽變成「已結束」。 |

## C. 帳號排除

| 編號 | 步驟 | 預期結果 |
|---|---|---|
| C1 | 有兩個可派工 Profile（A、B）時，以 A 開啟 Controller CLI，再從「工作」頁簽按「派工」派出一個 Task。 | Task 的 Run 狀態列顯示「帳號 B」；Controller 開著期間派出的 Worker 都不使用 A。 |
| C2 | 只有一個可派工 Profile（A）時，以 A 開啟 Controller CLI，再派出一個 Task。 | Task 不啟動，卡片顯示「排隊中」，「帳號」類別下的原因是「帳號「A」是開著的 Controller CLI 的 Controller 帳號，結束後才會派新的工作給它」。 |
| C3 | 接著在 Controller 頁簽按「結束」。 | 頁簽變成「已結束」後數秒內，排隊的 Task 以帳號 A 開始執行。 |
| C4 | 先以 A 派出一個 Worker（Controller 沒開），執行中再以 A 開啟 Controller CLI。 | 既有的 Worker 照常執行到結束，沒有被停止或交接；之後新派的 Task 不使用 A。 |

## D. 完全退出

| 編號 | 步驟 | 預期結果 |
|---|---|---|
| D1 | Controller CLI 執行中時，關閉 App 主視窗（收到系統匣），在 Controller CLI 中繼續對話並呼叫一個 Baton 工具（例如請 Controller 讀收件匣，`read_inbox`）；再從系統匣按「開啟 Baton」。 | 關閉主視窗不影響 Controller CLI，工具照常可用；重新開啟後 Controller 頁簽仍是「執行中」。 |
| D2 | Controller CLI 執行中時，從系統匣按「完全退出」。 | 出現「完全退出」進度，列出「<Workspace 名稱> · Controller CLI」，結果為「已停止」或「已強制終止」；Controller CLI 的視窗關閉，工作管理員中這個 Controller 的 `claude` 與 `baton-mcp-shim`（宿主）都已結束；最後 App 關閉。Windows Terminal 若保留分頁，比照 A7 記為發現事項。 |
| D3 | 重新開啟 App，進入同一個 Workspace 的 Controller 頁簽。 | 狀態是「已結束」，顯示「結束原因：完全退出時由 Runtime 結束」；不會自動重新開窗。 |

## E. 以既有目錄建立 Profile

| 編號 | 步驟 | 預期結果 |
|---|---|---|
| E1 | 在平台外已登入的設定目錄（例如 `C:\Users\<使用者>\.claude-Work-1`）準備好後，到「帳號」頁簽的「以既有目錄建立 Profile」表單輸入名稱與完整的設定目錄路徑，按「以既有目錄建立」。 | 不開登入終端機、不必重新登入；清單出現新的 Profile，狀態「可派工」，標示「外部目錄：<路徑>」，並顯示「認證來源：原生登入；帳號：<該目錄登入的帳號>；最近核對：…」。若顯示「尚未核對」，按「重新核對」並記為發現事項。 |
| E2 | 設定目錄改輸入 `%USERPROFILE%\.claude-Work-1` 或 `~\.claude-Work-1`。 | 平台不展開 `~` 與環境變數：被拒絕，顯示「設定目錄必須是完整的絕對路徑（例如 C:\Users\me\.claude-Work-3）：…」，沒有建立 Profile，輸入保留。 |
| E3 | 再以同一個目錄（可改大小寫或結尾加 `\`）、以及它底下的子目錄各建立一次。 | 兩次都被拒絕，顯示「設定目錄 <路徑> 與其他 Profile 的設定目錄相同或互相包含；每個 Profile 必須有獨立的設定目錄」，沒有建立 Profile。 |
| E4 | 在 Controller 頁簽以 E1 的 Profile 開啟 Controller CLI，輸入 `/status`。 | 顯示的帳號與帳號頁簽上這個 Profile 的帳號相同。按「結束」關閉。 |
| E5 | 在帳號頁簽按這個 Profile 的「封存」（無法復原），再到檔案總管查看該目錄；最後以同一個目錄再建立一次。 | Profile 從清單消失；目錄與其中的登入資料仍在，平台外以該目錄使用 `claude` 仍是登入狀態；再建立時以「相同或互相包含」被拒絕（已封存 Profile 的目錄不能重新登記）。 |

## F. 自動化部分

會開出真實的主控台視窗，不納入一般 `cargo test`：

```sh
cargo test -p baton-runtime --test controller_terminal -- --ignored
```

這個測試以新主控台執行 `cmd.exe /c ping -n 30 127.0.0.1`，確認新的 `WindowsProcessControl` 能以 `find` 找回同一個程序身分、`inspect` 為 Alive、`stop` 後為 Dead，且同一個 `command_id` 再開一次被拒絕。不開視窗的拒絕路徑（`command_id` 已有 Job、`command_id` 不合法）由同一個檔案在一般 `cargo test` 中執行。
