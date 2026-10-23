# AI 決策紀錄：desktop-controller-cli

本檔收錄 AI 在拆票與實作期間代為決定的事項。使用者會依序查看，再插入修正。

- spec 階段的產品與架構決策記在 [grilling.md](grilling.md)（Q1～Q20）與 [ADR-0014](../../docs/adr/0014-native-cli-controller.md)，本檔不重複。
- 本檔記錄拆票、執行方式與實作期間的決策，編號為 AI-NN，只能往後追加。
- **重要度**：
  - **高**：做錯了會讓帳號額度、安全、資料或大量工作白費，請優先查看。
  - **中**：影響工作方式或範圍，事後修改有一定成本。
  - **低**：預設值或慣例，改起來便宜。
- 每個條目都有一行 `- 審閱：未審｜認同 YYYY-MM-DD｜已修正`。新條目一律寫 `- 審閱：未審`；找未審條目用 `grep -n "^- 審閱：未審"`。
- 使用者表示同意時，把審閱行改成 `- 審閱：認同 YYYY-MM-DD`。
- 使用者修正時，把審閱行改成 `- 審閱：已修正`，並在條目下方加一行 `使用者修正：…`；AI 看到後依修正調整，並在需要時另開新條目說明影響。

## 條目格式

```markdown
### AI-NN：<主題>
- 重要度：高｜中｜低
- 時間／階段：YYYY-MM-DD，<拆票｜票 NN 實作｜驗證…>
- 決策：…
- 理由：…
- 替代方案：…
- 影響：…
- 審閱：未審
```

---

### AI-01：整合分支從 rebuild/v1 分出，命名 cli/v1
- 重要度：中
- 時間／階段：2026-10-23，拆票
- 決策：本 feature 的整合分支是 `cli/v1`，從 `rebuild/v1` 目前的 HEAD 建立；票分支 `cli/<票檔名>`；worktree 為 `issue-orchestrator-wt/cli-<NN>`；最終 review 以 `rebuild/v1` 為基準。
- 理由：本 feature 依賴 platform-rebuild-v1 的成果，而 `rebuild/v1` 還沒進 `main`；用前綴避免與 platform-rebuild-v1 的分支和 worktree 撞名。
- 替代方案：直接在 `rebuild/v1` 上繼續整合——兩個 feature 的進度與審查混在一起，難以分開驗收或回退。
- 影響：`rebuild/v1` 之後若再有修正，要手動合進 `cli/v1`。
- 審閱：未審

### AI-02：腳本與範本複製一份，共用 usage.py 與 normalize_decisions.py
- 重要度：低
- 時間／階段：2026-10-23，拆票
- 決策：把 platform-rebuild-v1 的 `.orchestration` 腳本與範本複製到本 feature，只把路徑、分支名與 review 基準換掉；額度查詢與決策正規化腳本直接引用 platform-rebuild-v1 的檔案。
- 理由：腳本裡寫死了 feature 路徑與分支；共用那兩支與 feature 無關的腳本，避免兩份實作分歧。
- 替代方案：把腳本參數化成兩個 feature 共用——要修改 platform-rebuild-v1 正在使用的 `dispatch.sh`，有截斷執行中 run 的風險。
- 影響：修 platform-rebuild-v1 腳本的 bug 時要記得同步這份。
- 審閱：未審

### AI-03：帳號並行數要跨 feature 加總
- 重要度：高
- 時間／階段：2026-10-23，拆票
- 決策：計算每個 Work-n 的進行中 run 數時，把 platform-rebuild-v1 與本 feature 的 `runs.md` 一起算，仍維持每帳號 < 3。
- 理由：「每帳號進行中 run < 3」是保護帳號額度的規則，不應因為分成兩個 feature 而變成 < 6。
- 替代方案：各 feature 各自計算——同一帳號可能同時跑到 5 個 run。
- 影響：主控每次派工要多讀一份總表。
- 審閱：未審

### AI-04：拆成 9 張票，以 Controller CLI 開啟／結束為 tracer bullet
- 重要度：中
- 時間／階段：2026-10-23，拆票
- 決策：01 頁簽外殼、02 以既有目錄建立 Profile、03 Controller CLI 開啟與結束（tracer bullet，所有後續票的基礎）、04 操作手冊／Worker 通則／AI 決策紀錄、05 帳號排除與換帳號重開、06 事件等待、07 完全退出與當掉恢復、08 移除舊對話、09 E2E 與 Windows 手動驗收。01、02 可立即開始；04～08 只依賴 03。
- 理由：03 一張就打通 Domain、Port、Runtime、shim、API 與 UI，之後每張都是在這條路上加一個可獨立驗證的行為；移除舊對話放在新路徑可用之後，避免中間有段時間沒有 Controller。
- 替代方案：依層拆（先 Domain、再 API、再 UI）——每張票無法單獨驗證；把 03 再拆細——開窗與宿主回報分開後，前一張無法 demo。
- 影響：03 是最大的一張；如果實作 run 撐不住，主控可把「宿主回報」切成新票。
- 審閱：未審

### AI-05：access token 全部過期時，以極小的 claude -p 請求刷新後再查額度
- 重要度：中
- 時間／階段：2026-10-23，第一次派工前
- 決策：`usage.py` 對所有帳號都回 `err HTTPError 401 token expired`（access token 過期，不是額度用盡；`claude auth status` 不會刷新）。主控對 Work-0 以外的每個帳號各送一次 `claude -p --model claude-haiku-5-5 --max-turns 1`（清理環境、`CLAUDE_CONFIG_DIR` 指向該帳號、工作目錄在 repo 外），讓 CLI 自行以 refresh token 換新，再重跑 `usage.py` 依門檻判斷。Work-0 不刷新、不派工。
- 理由：照 platform-rebuild-v1 的 AI-05，`err` 視為不可用；但全部帳號都 `err` 時永遠派不出去，而原因只是 token 久未使用而過期。極小請求的額度消耗可忽略，且由 CLI 自己寫回憑證，不必由主控改寫 `.credentials.json`。
- 替代方案：主控直接呼叫 OAuth refresh 端點並改寫憑證檔——寫壞會讓帳號需要重新登入；停下來等使用者手動處理——整個 feature 卡住。
- 影響：本次刷新後 Work-1 顯示 7d 已用 100%（不可用），Work-3／8／9 可用。之後若再遇到全數 401 token expired，照同樣方式處理。
- 審閱：未審

### AI-06：封存後的外部目錄不能再登記成新的 Profile
- 重要度：中
- 時間／階段：2026-10-23，票 02 實作
- 決策：以既有目錄建立 Profile 時，與任何 Profile（含已封存）的設定目錄相同或互相包含都拒絕（`profile_config_dir_shared`）；外部目錄的 Profile 封存後，同一個目錄也不能再登記。
- 理由：沿用平台既有規則「已封存的 Profile 目錄不重用，因為登入資料還在那裡」，資料庫也有 `config_dir` 不分大小寫唯一的索引；放寬要改 migration 與既有 Domain 規則，超出本票範圍。
- 替代方案：只和使用中的 Profile 比對，允許封存後重新登記同一目錄——要移除唯一索引，並決定封存紀錄與新 Profile 共用目錄時的身分與額度歸屬。
- 影響：使用者封存 `~/.claude-Work-n` 的 Profile 後想再加回來，目前做不到（也沒有「取消封存」）；若需要，另開票做「取消封存」或放寬規則。
- 審閱：未審

### AI-07：外部目錄與平台 profiles 目錄「重疊」就拒絕，含其上層目錄
- 重要度：低
- 時間／階段：2026-10-23，票 02 實作
- 決策：外部目錄位於 `<資料目錄>\profiles` 之內、等於它、或是它的上層（例如資料目錄本身）都拒絕（`profile_config_dir_managed`，400）；比對方式與 Profile 間的重疊相同（不分大小寫、忽略斜線方向、結尾分隔線與 `\\?\` 前綴）。路徑必須是絕對路徑（`profile_config_dir_invalid`），不展開 `~`。目錄不存在或不是目錄分別回 `profile_config_dir_not_found`、`profile_config_dir_not_directory`，平台不替使用者建立目錄。另外把既有的 `profile_config_dir_shared` 訊息改為「相同或互相包含」，平台建立的 Profile 也用同一句。
- 理由：spec 只寫「不在 profiles 底下」，但包含 profiles 的上層目錄一樣會讓之後平台建立的 Profile 落在它裡面，兩邊共用登入狀態。
- 替代方案：只擋「在 profiles 之內」——之後新增平台 Profile 時才被 `profile_config_dir_shared` 擋下，原因難懂。
- 影響：無。
- 審閱：未審

### AI-08：立即核對失敗時仍建立 Profile，顯示為尚未核對
- 重要度：低
- 時間／階段：2026-10-23，票 02 實作
- 決策：以既有目錄建立 Profile 的端點在登記成功後立即核對；核對本身失敗（例如 `claude` 沒有回應）時仍回 201，Profile 顯示「尚未核對」，使用者可按「重新核對」。只有目錄或名稱等驗證不通過才拒絕且不建立任何東西。
- 理由：核對失敗多半是暫時性的（CLI 沒回應），與目錄是否合格無關；回錯誤卻已建立 Profile 會讓使用者以為沒建立而重送，撞上目錄重疊。
- 替代方案：核對失敗就整筆回滾——要在 Store 增加刪除 Profile 的路徑，違反事件只追加。
- 影響：回應不帶核對失敗的原因；使用者按「重新核對」時才會看到。
- 審閱：未審

### AI-09：以獨立端點與文字輸入實作，不做原生資料夾選擇器
- 重要度：低
- 時間／階段：2026-10-23，票 02 實作
- 決策：新增 `POST /api/v1/profiles/from-existing-dir`（`{ name, config_dir }`）、Tauri 指令 `profile_create_from_dir`；Profile 多一個 `config_dir_origin`（`managed`／`external`）欄位。帳號畫面以獨立表單、文字輸入完整路徑；外部目錄的 Profile 在清單標示「外部目錄：路徑」。不接受 `expected_account`，身分以第一次核對到的原生登入固定（與平台建立的 Profile 相同）。封存的行為兩種來源都相同（平台本來就不刪設定目錄），差別只記在 `config_dir_origin` 上。
- 理由：spec 只要求「輸入目錄路徑與名稱」；原生資料夾對話框需要額外的 Tauri plugin 與權限設定，超出本票。
- 替代方案：沿用 `POST /api/v1/profiles` 加選填 `config_dir`——同一端點兩種語意（建立目錄與否），錯誤碼也難區分。
- 影響：之後若要加資料夾選擇器，只動前端表單與 Tauri 設定。
- 審閱：未審

### AI-10：全畫面首次精靈的條件、內容與「先進入主畫面」
- 重要度：中
- 時間／階段：2026-10-23，票 01 實作
- 決策：App 以 `GET setup` 判斷，沒有任何「使用中且可派工（`dispatchable`）」的 Account Profile 時，以全畫面顯示首次精靈＋帳號頁（沒有頁簽與側欄）；讀不到首次設定狀態時照常顯示頁簽。全畫面下方有「先進入主畫面」，只在這次開啟有效。有可派工 Profile 後，首次精靈改放在「帳號」頁簽最上方。
- 理由：`dispatchable` 只反映登入與身分核對（不含額度），最貼近 spec「可用 Account Profile」；精靈的文字要使用者在「下方帳號」新增與核對 Profile，全畫面若沒有帳號頁就無法完成設定。所有帳號登入暫時失效時若沒有出口，使用者連 Workspace 與裁定都看不到；冒煙腳本（`scripts/smoke-desktop.mjs`）的 Profile 也從未登入，需要這個出口才能走到 Workspace 與設定頁簽。
- 替代方案：改成「沒有任何 Profile」才全畫面——與 spec 的「可用」不符；維持無出口——冒煙腳本的平台狀態、授權與恢復三段失去 UI 覆蓋，且登入失效時整個 App 被鎖住。
- 影響：多一顆票面沒提到的按鈕；若使用者不要這個出口，移除按鈕並把冒煙腳本改成以 API 檢查那三段。
- 審閱：未審

### AI-11：「新選 Workspace 預設 Controller」的解讀
- 重要度：低
- 時間／階段：2026-10-23，票 01 實作
- 決策：從「沒有選中任何 Workspace」到選中一個（包括在側欄建立後自動選中）時，功能頁簽重設為 Controller；在兩個 Workspace 之間切換時停在同一頁簽。建立 Workspace 後因此不再直接進入授權設定，UI E2E 與冒煙腳本改為建立後點「授權」頁簽。
- 理由：票面同時要求「切換時停在同一頁簽」與「新選 Workspace 預設 Controller」，只有把「新選」解讀為「先前沒有選中」兩者才不衝突。
- 替代方案：建立新 Workspace 時一律回 Controller（或授權）——要另外區分「建立」與「切換」兩種選取，規則較難預期。
- 影響：使用者建立 Workspace 後要自己切到「授權」頁簽確認授權；票 03 填好 Controller 頁簽後可再評估是否在那裡提示「尚未確認授權」。
- 審閱：未審

### AI-12：頁簽配置細節
- 重要度：低
- 時間／階段：2026-10-23，票 01 實作
- 決策：「知識」頁簽把知識與專案理解上下疊放、「授權」頁簽把授權設定與模型設定上下疊放；只有「審查與驗證」用小頁簽（不記住所選的小頁簽）。左側 Workspace 清單只在 `Workspace` 頂層頁簽顯示。所在位置以 localStorage 的 `baton.shell` 保存頂層頁簽、Workspace id 與功能頁簽；上次的 Workspace 已不在清單（例如已封存）時顯示「請從左側選擇或建立 Workspace」。「裁定」頁簽的名稱固定為「裁定」，待裁定數以數字徽章顯示、title 為「待裁定 N」。
- 理由：票面只對「審查與驗證」指定小頁簽，知識與授權寫的是「＋」；舊畫面的授權與模型設定本來就疊在一起。頁簽名稱固定，E2E 才能以完全比對點選，也不會與側欄的「待裁定 N」文字重複比對。
- 替代方案：知識與授權也用小頁簽——多一層點選；側欄常駐於三個頂層頁簽——帳號與設定頁簽會出現與其無關的 Workspace 清單。
- 影響：知識頁簽較長；之後若要改成小頁簽，只動 `App.tsx` 的 `WorkspaceView`。
- 審閱：未審

### AI-13：票 01 的 AI-06～AI-08 順延為 AI-10～AI-12，連同驗證紀錄裡的引用
- 重要度：低
- 時間／階段：2026-10-23，票 01 整合
- 決策：票 02 先合併進 `cli/v1`，已占用 AI-06～AI-09；本票原本的 AI-06（全畫面首次精靈）、AI-07（新選 Workspace 預設 Controller）、AI-08（頁簽配置細節）依序改為 AI-10、AI-11、AI-12，內容不變，排在票 02 的條目之後。`runs/R004-01-verify.result.md` 是票 01 的驗證結論，裡面的 AI-06～AI-08 也同步改為 AI-10～AI-12；`R003-02-verify.result.md` 引用的是票 02 自己的編號，不動。git 自動合併時把兩邊的條目交錯接在一起（本票 AI-08 少了審閱行），所以以 `cli/v1` 的版本為底重新接上本票條目，而不是採用自動合併的結果。
- 理由：整合分支的條目不能修改，只能由後合併的票順延；驗證紀錄會被修正 run 讀取，留舊編號會指到票 02 的條目。
- 替代方案：驗證紀錄維持原樣當作歷史——之後讀到「建議使用者審 AI-06」會找錯條目。
- 影響：commit 93b1324 的訊息仍寫 AI-06～AI-08，以本檔為準。
- 審閱：未審

### AI-14：互動式 Controller Run 與 Controller 的一輪共用 controller_run 資料表
- 重要度：中
- 時間／階段：2026-10-23，票 03 實作
- 決策：互動式 Controller Run 存在既有的 `controller_run` 資料表，以新欄位 `kind`（`turn`／`cli`）區分；migration `controller_cli_run` 重建資料表，讓 CLI 的列可以沒有 conversation、Prompt、輸入與 Agent Session，狀態多一個 `ended`，並加上「每個 Workspace 最多一個未結束的 CLI」唯一索引。重建時以 `PRAGMA legacy_alter_table` 改名，其他資料表以名稱參照 `controller_run` 的觸發器與外鍵維持不變。對話相關的查詢都只讀 `kind = 'turn'`。
- 理由：Controller 憑證（`credential.controller_run_id` 外鍵）、Controller 提出的裁定請求（觸發器檢查 `controller_run`）、Controller 寄出的訊息（外鍵）都以 `controller_run` 為範圍；共用資料表，Controller CLI 的憑證不必改任何既有機制就能使用 Controller 工具，Run 結束（`ended`）時憑證自然失效。
- 替代方案：另開 `controller_cli_run` 資料表——憑證、裁定請求與訊息的外鍵與觸發器都要重建或改成兩表擇一，改動面更大。
- 影響：票 08 移除舊對話時保留 `kind = 'turn'` 的資料即可；之後查詢 `controller_run` 的新程式要記得依 `kind` 區分。
- 審閱：未審

### AI-15：以新主控台開窗，交給 Windows 的預設終端機設定，不直接呼叫 wt.exe
- 重要度：中
- 時間／階段：2026-10-23，票 03 實作
- 決策：正式的「開啟 Controller 終端機」以 `CREATE_NEW_CONSOLE` 建立宿主程序（與 Worker 相同的具名 Job），視窗由 Windows「預設終端機應用程式」設定決定：Windows 11 預設是 Windows Terminal，沒有時是系統主控台。不直接執行 `wt.exe`。視窗標題是「Baton Controller - <Workspace 名稱>」。
- 理由：spec 寫「有 Windows Terminal 就用它」。但 `wt.exe` 會把命令交給已在執行的 Windows Terminal 實例，不保證帶上呼叫端的環境變數；Controller 憑證與 `CLAUDE_CONFIG_DIR` 只經環境變數交付，帶不過去就會開出錯的帳號或沒有工具的 CLI。直接建立程序也才拿得到宿主的程序身分並放進可找回的 Job（不重複開窗、查驗存活、結束整棵程序樹都靠它）。
- 替代方案：呼叫 `wt.exe -w new`，憑證改寫進暫存檔交給宿主——憑證落地成檔案，且仍拿不到宿主身分。
- 影響：使用者把預設終端機設成系統主控台時不會用 Windows Terminal；Windows 10 上要使用者自行把 Windows Terminal 設為預設終端機。實機行為列在 `docs/validation.md` 的手動驗收。
- 審閱：未審

### AI-16：Controller CLI 載入 Workspace 的專案層設定、保留 Baton 工具免逐次確認、不套用模型覆寫
- 重要度：中
- 時間／階段：2026-10-23，票 03 實作
- 決策：Controller CLI 與 Worker 共用組裝（環境清理、`CLAUDE_CONFIG_DIR`、`--strict-mcp-config` 指向 shim 與共同設定的 MCP、設定快照的 `--settings`／`--append-system-prompt-file`），差別：不加 `-p`、`--output-format`、`--verbose`、`--permission-prompt-tool`；`--setting-sources` 用 `project,local`（Worker 是空字串）；保留 `--allowedTools mcp__baton`；不帶 Workspace 的模型覆寫。
- 理由：使用者故事 23、58 要求 Workspace 的 `CLAUDE.md` 被原生載入，空的設定來源會把它關掉；使用者層的設定由共同設定快照提供，不再讀帳號目錄裡的 settings.json，避免兩份重複。`--allowedTools` 不是權限模式，只讓 Baton 自己的工具不必每次確認，其他操作仍照 Claude Code 預設互動詢問。模型覆寫不在票 03 範圍。
- 替代方案：照 Worker 用空的設定來源——Workspace 的 `CLAUDE.md` 不會載入；連 `--allowedTools` 也拿掉——每次派工、查詢都要使用者確認。
- 影響：Workspace 內的 `.claude\settings*.json` 會影響 Controller CLI 的權限（使用者在場，可接受）；若要套用 Controller 角色的模型覆寫，另開票。
- 審閱：未審

### AI-17：環境清理加入 AI_AGENT 與 BAGGAGE，Worker 一併適用
- 重要度：低
- 時間／階段：2026-10-23，票 03 實作
- 決策：啟動 Claude Code 前移除的繼承環境變數，除了原有的 `ANTHROPIC*`、`CLAUDE*`（保留 `CLAUDE_CODE_GIT_BASH_PATH`）之外，加入 `AI_AGENT`、`BAGGAGE`；Worker 與 Controller CLI 用同一份清單（`inherited_to_clear`）。語意 judge 的呼叫維持原本只清認證相關變數。
- 理由：spec 的共用組裝明列這兩個變數；在外層 Claude Code 裡啟動 Runtime 時它們會被帶進子程序，影響 CLI 的行為與遙測歸屬。
- 替代方案：只對 Controller CLI 清除——兩種啟動的組裝不再相同。
- 影響：Worker 的 `env_remove` 多了這兩個名稱（只在 Runtime 繼承到時）。
- 審閱：未審

### AI-18：開啟分兩步（要求→launch intent），running 以開窗回執或宿主回報先到者為準，失敗以結束代碼區分
- 重要度：中
- 時間／階段：2026-10-23，票 03 實作
- 決策：`POST .../controller-cli` 在交易中檢查 Workspace、Profile 與「沒有未結束的 CLI」後建立 `starting` 的 Run（202）；Supervisor 開窗前另外寫下 launch intent（`launched_at`），寫完才開窗。重啟後看到「有 launch intent、沒有回執」就以 command_id 找回宿主，找到補寫回執、找不到以 `launch_outcome_unknown` 結束，不重新開窗。開窗回執或宿主回報 `claude` 已啟動，先到的那個讓 Run 進入 `running`。「結束」也只記下要求，由 Supervisor 停止宿主（寬限 2 秒後終止整棵程序樹）。開啟失敗一律結束 Run，以 `end_code`（`claude_not_found`、`profile_unavailable`、`terminal_failed`、`launch_failed`、`launch_outcome_unknown`）與繁中 `end_reason` 呈現；Profile 在要求之後、開窗之前失效也以 `profile_unavailable` 結束。開窗前先以既有的 `probe_claude` 確認找得到 `claude`。
- 理由：API 沒有程序控制與憑證簽發，由 Supervisor 統一開窗與核對才能沿用 Worker 的「先持久化再啟動、以 command_id 找回」模式；分出 launch intent 才分得出「還沒開」與「開了但結果不明」。
- 替代方案：API 同步開窗——API 與 Supervisor 會同時處理同一個 starting 的 Run，崩潰核對可能誤判。
- 影響：頁簽需要輪詢才看得到 `running`／失敗原因（最慢約一秒，背景迴圈每秒處理一次）。
- 審閱：未審

### AI-19：Controller 宿主的行為細節
- 重要度：中
- 時間／階段：2026-10-23，票 03 實作
- 決策：`baton-mcp-shim controller-host <program> [args]`：宿主先把自己放進「宿主結束時一併終止」的 Job 再啟動 `claude`（宿主被結束或視窗被關時 `claude` 一起結束）；以處理函式忽略 Ctrl+C／Ctrl+Break（不被子程序繼承，`claude` 照常收到）；啟動後回報 `POST /agent/v1/controller-host/started {claude_pid}`，回 401（平台已結束這個 Run）就結束 `claude` 並以結束碼 3 結束；`claude` 結束後回報 `ended {exit_code}` 並以同一個結束碼結束；啟動失敗時回報 ended、提示「按 Enter 關閉視窗」並以 1 結束；設定缺漏以 2 結束。回報的網路失敗只警告，存活另由 Runtime 查驗。
- 理由：讓 Runtime 不是終端機程序的父程序也能追蹤生命週期；`claude.cmd` 經 cmd.exe 啟動時只結束 cmd 會留下真正的 `claude`。
- 替代方案：宿主不建 Job——從 App 結束時只靠外層 Job，宿主被工作管理員單獨結束時會留下 `claude`。
- 影響：從 App 按「結束」時 `claude` 是被終止的（不是 `/exit`），Windows Terminal 可能依 `closeOnExit` 設定保留分頁並顯示結束碼。
- 審閱：未審

### AI-20：時間線新增「controller」類別
- 重要度：低
- 時間／階段：2026-10-23，票 03 實作
- 決策：時間線新增類別 `controller`，收錄聚合 `controller_cli` 的事件：`controller_cli_opened`（含 `profile_id`、`profile_name`）、`controller_cli_launch_started`、`controller_cli_running`、`controller_cli_end_requested`、`controller_cli_ended`（含 `end_code`、`end_reason`、`exit_code`、`profile_id`）。事件不屬於任何 Task，以 `controller_run` 的 Workspace 歸屬。
- 理由：使用者故事 41 要求時間線記錄 Controller CLI 的開啟、結束與帳號。
- 替代方案：併入既有的「派工」類別——會混進派工的觀測指標計算。
- 影響：前端時間線的類別標籤多一個「Controller」。
- 審閱：未審

### AI-21：Controller CLI 開窗的手動驗收先放在 docs/validation.md
- 重要度：低
- 時間／階段：2026-10-23，票 03 實作
- 決策：真實開窗以 `crates/baton-runtime/tests/controller_terminal.rs` 的 `#[ignore]` 測試（開出真實視窗、以 Job 找回並停止）與 `docs/validation.md` 檔尾新增的「Windows 平台手動驗收：Controller CLI 開窗（票 03）」表格驗收；一般 `cargo test` 只跑不開視窗的拒絕路徑。
- 理由：repo 內沒有 Windows 平台驗收清單（原清單在 GitHub issue #3 §15），票面允許「明確標為手動驗收」。
- 替代方案：把開窗測試納入一般測試——每次 `cargo test` 都會跳出視窗。
- 影響：票 09 整理 Windows 驗收清單時，把這一節搬過去。
- 審閱：未審

### AI-22：Controller 頁簽的呈現細節
- 重要度：低
- 時間／階段：2026-10-23，票 03 實作
- 決策：頁簽顯示最近一次 Controller Run：狀態（未開啟／開啟中／執行中／已結束／中斷）、Controller 帳號、開啟時間；開著時以「Controller CLI 已開啟，請切到它的終端機視窗」取代「開啟」（不做「切到視窗」），「結束」按下後顯示「結束中…」。帳號下拉只列使用中的 Profile，不可派工的停用並在下方「不可用的帳號」列出完整原因，預設選第一個可用的。開啟失敗的結束代碼顯示「開啟失敗：原因」，其餘顯示「結束原因：原因」。開著時每 2 秒重新讀取；帳號清單只在進入頁簽時讀一次。錯誤訊息沿用其他頁簽的顯示方式（Tauri 下帶「Runtime 拒絕連線（代碼）」前綴）。
- 理由：使用者故事 29 允許「提示已開啟」；切到既有視窗需要視窗管理的 API，超出本票。
- 替代方案：實作「切到已開啟的視窗」——要以宿主程序找出所屬的終端機視窗，Windows Terminal 下拿不到可靠的視窗代碼。
- 影響：在帳號頁簽核對完 Profile 後要重新切回 Controller 頁簽才會更新選單；票 05 加額度顯示時可一併改為輪詢。
- 審閱：未審

### AI-23：重啟後宿主仍存活時，Run 維持執行中、只讓舊憑證失效，不替使用者結束 CLI
- 重要度：中
- 時間／階段：2026-10-23，票 07 實作
- 決策：spec 的「憑證世代加一」實作為 Controller Run 上的 `rebound_at`（新欄位，migration `controller_cli_recovery`）：Runtime 重啟後以 command_id 找到宿主仍存活時寫入它，並記一筆 `controller_cli_rebound` 事件；`rebound_at` 有值時，以這個 Run 簽發的 Controller 憑證一律回 401（`controller_run_active` 多檢查這一欄），宿主的開始／結束回報也被拒。Run 維持 `running`，Runtime 不停止宿主；頁簽提示「請按『結束』後以同一個帳號重新開啟」。使用者關掉終端機時由既有的存活查驗結束 Run（`host_gone`）。不另外簽發新的憑證。
- 理由：憑證只經環境變數交給宿主，已開著的 CLI 拿不到新的憑證，所以「重新綁定」實際上就是讓舊憑證失效、等使用者重開；使用者可能正在和那個 CLI 對話，Runtime 自己把它關掉會打斷工作。憑證資料表沒有世代或撤銷欄位，以 Run 上的一個時間欄位達成相同效果，改動最小。
- 替代方案：在憑證與 `ControllerScope` 加世代數字——要改憑證資料表與所有 Controller 工具的範圍檢查，效果相同；重啟後直接停止宿主並標為中斷——會中斷使用者正在進行的互動。
- 影響：重新綁定後到使用者重開之前，那個 CLI 的平台工具都不能用（票 06 的 `wait` 也會以憑證失效結束）；同一個 Workspace 在使用者按「結束」或關掉終端機之前不能開新的 Controller CLI。
- 審閱：未審

### AI-24：每次 Runtime 啟動都查驗上一個實例開出的 Controller CLI，不只當掉恢復
- 重要度：中
- 時間／階段：2026-10-23，票 07 實作
- 決策：Supervisor 記住「這個 Runtime 實例自己寫下 launch intent 或已查驗過」的 Controller Run；其他已寫過 launch intent 的非終態 Run 一律視為上一個實例開出的，先查驗：找到宿主→（沒有回執時補寫）重新綁定；有回執但找不到→`interrupted`（`end_code = runtime_restarted`）；沒有回執也找不到→沿用票 03 的 `ended`／`launch_outcome_unknown`。正常重啟（沒有經過完全退出）與當掉恢復一樣處理；不依賴崩潰恢復事件，也不受恢復暫停派工影響。
- 理由：舊實例交出的 Runtime 位址與憑證在新實例下不保證可用，正常重啟與當掉的差別只在於程序有沒有記下正常結束；以「是不是這個實例開出的」判斷不需要讀恢復狀態。開窗結果不明的情形票 03 已定義並通過驗證，沒有理由改成 `interrupted`。
- 替代方案：只在偵測到意外終止（有恢復事件）時查驗——正常重啟後 CLI 拿著舊憑證，行為取決於重啟方式，較難預期。
- 影響：Runtime 因更新等原因正常重啟後，開著的 Controller CLI 也要重新開啟。
- 審閱：未審

### AI-25：同帳號接續的判斷：最近一次真的執行過的 Run 用同一個帳號，開啟時決定並持久化
- 重要度：中
- 時間／階段：2026-10-23，票 07 實作
- 決策：使用者要求開啟時，Domain 找這個 Workspace 最近一次 `running_at` 有值（真的執行過）的 Controller Run；它的 Profile 與這次相同就把 `continue_session` 設為 true（新欄位，同一個 migration），Adapter 在 `claude` 參數加 `--continue`；換帳號、或從未執行過時不加。開啟失敗的 Run 不列入比對。`RunSpec` 多一個 `continue_session` 欄位（Worker 一律 false）。這條規則與票 05 的驗收條件重疊，本票先實作；Controller 頁簽未開啟時預設選最近一次 Run 的帳號（仍可用時），讓「同帳號重開」是預設動作。
- 理由：票 07 的「恢復後重新開啟同帳號時帶 `--continue`」與票 05 的「同帳號重開帶 `--continue`」是同一條規則，分兩處實作會不一致；持久化在 Run 上，Runtime 重建後組出相同的啟動指令（command hash 不變），能以 command_id 找回宿主。
- 替代方案：每次組啟動規格時才回頭查前一個 Run——規則藏在 Supervisor，API 與時間線看不到這次是否接續。
- 影響：票 05 合併時應沿用這個欄位與規則，不要再做一份。若上次的 CLI 開了就關、沒有留下原生 Session，`claude --continue` 可能顯示找不到可接續的對話，使用者要改用其他帳號或手動開新對話；平台目前不偵測這種情形。
- 審閱：未審

### AI-26：完全退出時 Controller CLI 的停止方式與在退出進度中的呈現
- 重要度：低
- 時間／階段：2026-10-23，票 07 實作
- 決策：完全退出的停止流程讀取所有開著的 Controller CLI，先以「等待中」列入停止報告（`ExitReport.runs`，`controller_cli: true`、`task_id` 為空字串），再與 Worker 同時停止宿主（寬限 2 秒，與按「結束」相同），同樣受 40 秒總上限約束。確認停止或從未開出宿主的以 `end_code = full_exit` 結束 Run；無法確認的記為停止異常、Run 維持原狀（重新開啟後由 AI-24 的查驗處理）。退出畫面以「Workspace 名稱 · Controller CLI」標示這一項。
- 理由：沿用既有的停止報告與停止異常呈現，不新增另一份進度；互動式 `claude` 不會因為寬限期而自行保存結束，給 Worker 的 30 秒寬限只會拖慢退出。
- 替代方案：另開「Controller CLI 停止結果」欄位——前端要多一套呈現，且停止異常的判斷要重寫。
- 影響：停止報告的項目不再都是 Worker Run；讀取 `runs` 的程式要以 `controller_cli` 區分（目前只有退出畫面）。只關 App 視窗不經過這個流程，Controller CLI 不受影響（Runtime 是獨立程序，沒有改動）。
- 審閱：未審

### AI-27：等待的事件來源是追加式事件表，游標就是事件序號；寄給 Controller 的訊息另追加收件匣事件
- 重要度：中
- 時間／階段：2026-10-23，票 06 實作
- 決策：等待端點直接從追加式 `event` 表讀出四種事件，游標是事件表的 `seq`：Worker Run 結束＝`run_succeeded`／`run_failed`／`run_cancelled`／`run_interrupted`（交接 `run_handed_off` 不算，工作由接手的 Run 繼續）；Task 受阻＝`task_status_changed` 且 `to = blocked`；裁定被回答＝該 Workspace 任何裁定請求的 `decision_answered`（含 Worker 的與權限提示）；收到訊息＝新事件 `controller_inbox`／`controller_message_received`（`aggregate_id` 是 Workspace，Worker 寄給 Controller 的訊息存入收件匣時在同一個交易中追加）。一次最多回 20 筆。舊對話的工作事件佇列（`enqueue_work_events`）不使用。
- 理由：事件表已在每個狀態變更的交易中寫入、只追加且有全域順序，作為游標最可靠；不用再在四條寫入路徑各加一份佇列。舊對話的佇列會在票 08 移除。訊息原本不產生事件，只能補一個。
- 替代方案：另建 `controller_event` 佇列表，四條寫入路徑各自寫入——改動面大，且與事件表重複。
- 影響：時間線不顯示 `controller_inbox` 事件（不在時間線聚合清單內）。Controller 自己取消 Task 也會收到 Run 結束通知。
- 審閱：未審

### AI-28：wait 印出摘要後才由 shim 確認，確認走同一個端點
- 重要度：中
- 時間／階段：2026-10-23，票 06 實作
- 決策：`POST /agent/v1/controller/wait`，body `{ ack?, timeout_seconds? }`。shim 的 `wait` 先送等待請求，把事件摘要寫到 stdout 並 flush 成功後，再送 `{ ack: <回應的 cursor>, timeout_seconds: 0 }` 確認；確認失敗只警告，事件下次會再回報（至少一次）。沒確認前再等一次會拿到同一批。確認只會往前；超過目前最後一筆事件或為負數回 400 `wait_cursor_invalid`。
- 理由：spec 要求 shim 不保存狀態，又要求「確認後同一事件不再回傳」；由 shim 在同一次執行內確認，Controller 不必自己記游標，也不會因為 wait 程序在印出前被結束而漏掉事件。
- 替代方案：伺服器回傳即視為確認——印出前程序被結束就漏事件；要 Controller 帶 `--ack <游標>`——Controller 忘了帶就會反覆收到同一批，背景 wait 一直立即結束。
- 影響：每次有事件的 wait 會打兩個請求。
- 審閱：未審

### AI-29：新的 Controller Run 接續同一個 Workspace 上一個 Controller 確認到的位置
- 重要度：中
- 時間／階段：2026-10-23，票 06 實作
- 決策：Controller Run 第一次等待時決定起點並保存：同一個 Workspace 先前的 Controller Run 確認過的最大游標；沒有的話是這個 Run 的 `controller_cli_opened` 事件；都沒有時是目前最後一筆。之後依 Controller Run 保存在新資料表 `controller_event_cursor`（只能往前）。
- 理由：換帳號重開、Controller 關著時 Worker 結束，新的 Controller 一掛上 wait 就該收到（spec「沒掛時事件只會累積，下次掛上時立即回傳」）；第一次使用時不回報開啟前的整段歷史，避免一開就被大量舊事件淹沒。
- 替代方案：每個 Run 都從自己開啟時開始——關著期間發生的事會被跳過；從 0 開始——舊 Workspace 一開就收到全部歷史。
- 影響：上一個 Controller 沒確認的事件會交給下一個 Controller。
- 審閱：未審

### AI-30：等待的逾時、批次與 wait 的結束碼
- 重要度：低
- 時間／階段：2026-10-23，票 06 實作
- 決策：逾時預設 600 秒、上限 3600 秒（超過回 400 `wait_timeout_invalid`；`--timeout` 格式錯或超過上限，shim 以 2 結束且不送請求），`0` 表示只查一次。結束碼：0 有事件、1 其他失敗（連線失敗、非 401 的錯誤回應、回應無法解析）、2 參數或環境變數錯誤、3 憑證失效（401，Controller CLI 已結束）、4 逾時。逾時說明印在 stdout，其餘錯誤走 stderr。Worker 憑證呼叫回 403 `controller_only`（shim 以 1 結束）。Runtime 停止時等待中的請求回 503 `runtime_stopping`。
- 理由：2、3 與 Controller 宿主子命令的結束碼意義一致；逾時要讓 Controller 能辨識並決定是否重掛。
- 替代方案：逾時用 124（GNU timeout 慣例）——與宿主的結束碼不成一組。
- 影響：操作手冊（票 04）需要說明這些結束碼。
- 審閱：未審

### AI-31：Controller 宿主以 BATON_MCP_SHIM 環境變數把 shim 執行檔路徑交給 claude
- 重要度：中
- 時間／階段：2026-10-23，票 06 實作
- 決策：`controller-host` 啟動 `claude` 時設定 `BATON_MCP_SHIM=<shim 執行檔完整路徑>`（取自 `current_exe`），Controller 以 `"$BATON_MCP_SHIM" wait` 在背景 Bash 執行等待。憑證與 Runtime 位址仍只經既有的環境變數繼承，不出現在命令列。
- 理由：shim 不在 PATH 上，Controller 沒有其他可靠方式找到它；由宿主自己設定，不必改 Runtime 的啟動組裝。
- 替代方案：把 shim 目錄加進 PATH——會改變使用者在 Controller CLI 裡執行其他指令的解析順序；由 Runtime 在啟動規格加變數——要改票 03 的共用組裝。
- 影響：票 04 的操作手冊應以 `"$BATON_MCP_SHIM" wait` 說明背景等待。
- 審閱：未審

### AI-32：長輪詢以 Store 的提交通知叫醒、以 Clock 計時
- 重要度：低
- 時間／階段：2026-10-23，票 06 實作
- 決策：Store 每次寫入交易提交就通知訂閱者，等待端點據此重新查詢（並重新檢查憑證，Controller 結束時回 401）；逾時以 Clock Port 新增的 `sleep_until` 計時。正式時鐘用 tokio 的 sleep；harness 的 fake Clock 只在測試推進時間時醒來，並提供 `wait_for_sleepers` 讓測試確認端點已經開始等待。`sleep_until` 的預設實作永遠不醒（只有 API 用的時鐘需要覆寫）。
- 理由：票面要求以 fake Clock 驗證、不用真實 sleep；提交通知讓事件一發生就回應，不必定時輪詢資料庫。
- 替代方案：每秒輪詢一次資料庫與時間——測試要等真實時間，事件也最多延遲一秒。
- 影響：每次寫入都會讓等待中的端點多查一次（只查游標之後的事件，本機負擔很小）。
- 審閱：未審

### AI-33：固定指示與共同指示合併成一個系統提示檔，Worker 的 `--append-system-prompt-file` 改指向 Run 目錄
- 重要度：中
- 時間／階段：2026-10-23，票 04 實作
- 決策：Claude Code 只帶一個 `--append-system-prompt-file`，內容是「固定指示＋（有的話）`---` 分隔＋共同設定的 `CLAUDE.md`」（`baton_domain::playbook::system_prompt`）。Worker：Adapter 的 `prepare` 寫 `runs\<run_id>\system-prompt.md`（Worker 通則＋快照的共同指示），`spawn_spec` 一律帶這個檔案，即使沒有共同設定。Controller CLI：Supervisor 以 `write_readonly_files` 把操作手冊（＋共同指示）寫成 `run-settings\<run_id>\system-prompt.md`，與設定快照同一個唯讀目錄；這個目錄現在一定存在，沒有共同設定版本時只放操作手冊。原本斷言 Worker 的 `--append-system-prompt-file` 指向快照 `CLAUDE.md`（或沒有共同設定時不帶這個旗標）的 4 處既有測試，改為斷言指向合併檔並檢查檔案內容含共同指示；其餘 Worker 啟動測試沒有改。
- 理由：`--append-system-prompt-file` 在 `claude --help` 中沒有列出，重複指定時是否都生效無從確認；同時用 `--append-system-prompt` 帶內嵌文字也可能被拒，而且經 cmd.exe 啟動 `claude.cmd` 時，換行與特殊字元會被 `prepare` 擋下。合併成一個檔案的行為最確定。
- 替代方案：重複指定兩個 `--append-system-prompt-file`——若只有最後一個生效，共同指示或通則就會悄悄消失；把通則放進 Prompt（標準輸入）——不是票面要求的系統提示。
- 影響：舊的 Controller 輪次（`claude -p`，票 08 會移除）也走 `spawn_spec`，在移除前同樣會帶 Worker 通則。升級前已在執行的 Worker 組出的啟動指令與現在不同（多一個旗標），只有在不完全退出就換版本時才會影響以 command_id 找回程序。
- 審閱：未審

### AI-34：操作手冊內建在 domain、版本號為 "1"，頁簽顯示 App 內建的版本
- 重要度：低
- 時間／階段：2026-10-23，票 04 實作
- 決策：操作手冊、Worker 通則與 AI 決策紀錄範本是 `crates/baton-domain/src/playbook/*.md`，以 `include_str!` 編進程式，並用該目錄的 `.gitattributes` 固定為 LF（repo 的 `core.autocrlf=true` 會讓不同 checkout 編進不同的換行）。`PLAYBOOK_VERSION = "1"`，手冊標題行是「# Baton Controller 操作手冊（版本 1）」，以測試確保兩者一致。`ControllerCliView` 新增 `playbook_version`（從未開啟過也有），Controller 頁簽的狀態欄列出「操作手冊版本」。版本號不記在每個 Run 上。「不含帳號名稱、額度數字、並行上限、worktree 路徑」以測試檢查：禁用字串（`Work-`、`.claude-`、`%`、`5h`、`7d`、`≥`、`-wt`、`../`、磁碟路徑等），並規定內文除了編號清單的序號，唯一的數字只能是「最多修正 3 次」。
- 理由：Run 的手冊檔是開啟時由同一個執行檔寫入的，版本必定等於內建版本；只有「Controller 開著時更新 App」才會不同，而更新要先完全退出，Controller CLI 也會一起結束（票 07）。
- 替代方案：在 `controller_run` 記錄開啟時的手冊版本——要加 migration，目前沒有使用情境。
- 影響：修改手冊內容時要記得調整 `PLAYBOOK_VERSION` 和標題（標題那一半有測試會擋）。
- 審閱：未審

### AI-35：Worker 通則不寫死 AI 決策紀錄的路徑，由 Controller 寫進 Task 契約
- 重要度：中
- 時間／階段：2026-10-23，票 04 實作
- 決策：Worker 的工作目錄是 Task 的 worktree，不是 Workspace 目錄，而 Worker 通則是固定文字，不知道 Workspace 在哪裡。所以通則寫「路徑寫在 Task 契約中；沒寫時在最後的回報中列出你的決定，由 Controller 代為追加」；操作手冊要求 Controller 派工時在契約的輸入寫明 `<Workspace>\.baton\ai-decisions.md` 的完整路徑，並代為追加 Worker 只在回報中列出的決定。
- 理由：spec 要求通則固定，又要求 Worker 把決定寫進 Workspace 的紀錄檔；由 Controller 補上路徑，兩者都能滿足。
- 替代方案：Adapter 依 Run 的 Workspace 把路徑插進系統提示——通則就不再固定，Adapter 也要多知道 Workspace 目錄；寫相對路徑 `.baton/ai-decisions.md`——在 worktree 裡會寫到錯的檔案（或寫進分支裡）。
- 影響：Worker 寫 Workspace 目錄要經過權限判定（Workspace 授權不允許時會被擋）；被擋時就照通則改在回報中列出。
- 審閱：未審

### AI-36：AI 決策紀錄的建立方式、範本與失敗處理
- 重要度：低
- 時間／階段：2026-10-23，票 04 實作
- 決策：Supervisor 寫下 launch intent、確認找得到 `claude` 之後、準備唯讀目錄之前，以新的 `WorkspaceFs::create_file_if_absent`（`create_new` 開檔，需要時建立 `.baton` 目錄）建立 `<Workspace>\.baton\ai-decisions.md`：已存在（包括檢查之後才被建立的）一律不動。建立失敗（例如 `.baton` 是一個檔案）時以 `launch_failed`、原因「無法建立 AI 決策紀錄 …」結束這次開啟，不開窗。範本照 `.scratch/platform-rebuild-v1/ai-decisions.md` 的檔頭：標題改為「# AI 決策紀錄」，說明改成 Workspace 適用的文字，拿掉「spec 決策記在 spec.md 的 D-01～D-33」那一行；重要度、審閱行規則、條目格式區塊與 `---` 逐字相同（有測試）。
- 理由：沒有紀錄檔時，手冊要求 Controller 追加的對象就不存在；直接以失敗呈現，使用者看得到原因。用 `create_new` 而不是「先讀再原子改名」，是因為改名會覆蓋檢查之後才出現的檔案。
- 替代方案：建立失敗時只記錄、照常開窗——Controller 之後追加也很可能失敗，問題會被延後、更難發現。
- 影響：寫到一半時當掉，可能留下內容不完整的範本檔，之後也不會被覆寫（檔案很小，機率極低）。
- 審閱：未審

### AI-37：操作手冊裡等待指令、受阻與決策權限表的寫法
- 重要度：中
- 時間／階段：2026-10-23，票 04 實作
- 決策：等待照票 06 的 AI-30、AI-31，寫成「以 Bash 工具的背景執行跑 `"$BATON_MCP_SHIM" wait`」，並說明結束碼：0 有事件就處理、4 逾時就重新掛上、3 憑證失效就不再掛並請使用者從 App 重新開啟、其他結束碼把錯誤告訴使用者。手冊不寫 `--timeout` 與逾時秒數，用預設值。Controller 工具沒有「把 Task 標為受阻」的專用工具，所以修正 3 次仍不通過時，手冊寫成「以 `update_task` 在契約的阻礙處理記下原因，不再派工」。決策權限表列出 D-15 中必須送裁定的事項，但不列「三次恢復失敗後的重試」（只能由使用者從恢復介面發起，Controller 不能提出）；不確定時先呼叫 `controller_action` 讓 Runtime 判定。驗證要「由新的 Worker Run 執行、不沿用實作的 Session」，因為帳號由 Runtime 選，Controller 無法指定不同的帳號。檢查手冊數字的測試，除了「最多修正 3 次」之外也允許「結束碼 N」。
- 理由：手冊不能要求 Controller 用平台沒有的工具；`wait` 的呼叫方式與結束碼已經由票 06 定案並合入 `cli/v1`。
- 替代方案：手冊也寫逾時秒數——那是 Runtime 的預設值，寫進手冊就要跟著改。
- 影響：手冊還沒發佈過，所以這次修改仍是版本 1。之後若改 `wait` 的介面或新增「標記受阻」的 Controller 工具，要改手冊並升版。
- 審閱：未審

### AI-38：升級前的對話資料原樣保留，Controller 憑證只認 Controller CLI
- 重要度：中
- 時間／階段：2026-10-23，票 08 實作
- 決策：`conversation`、`conversation_turn`、`kind = 'turn'` 的 `controller_run` 與 Controller Session 不加 migration、不改狀態、不刪除；Runtime 不再讀寫它們（含升級時停在 `pending` 的輸入與停在 `starting`／`running` 的舊輪次）。Controller 憑證的有效性檢查（`controller_run_active`）改為只認 `kind = 'cli'`，所以舊輪次的憑證一律被拒。完全退出不再停止舊輪次的程序。桌面 API 的兩個對話端點、`baton-client` 的對應方法與 Tauri 指令直接移除（回 404），不保留唯讀檢視。
- 理由：票面要求「保留、不遷移、不再寫入」；把殘留的舊輪次標成 interrupted 本身就是寫入。舊輪次的憑證若仍因狀態停在 running 而有效，會留下一個沒人管理卻能用 Controller 工具的憑證。
- 替代方案：加 migration 把未結束的舊輪次標為 interrupted——違反「不遷移」；保留唯讀的對話檢視——UI 已無入口，只是多一個要維護的端點。
- 影響：升級前若有舊輪次的 `claude -p` 程序仍在執行（Runtime 意外終止且程序沒被 Job 一併結束），平台不再追蹤它，它的工具呼叫會被拒；歷史對話只能直接查資料庫。
- 審閱：未審

### AI-39：所有寫入 conversation 的工作事件一併停止，Domain 中只為對話產生的輸出暫不刪除
- 重要度：中
- 時間／階段：2026-10-23，票 08 實作
- 決策：Store 不再把下列事項寫成 conversation 的工作事件：Worker Run 成功／失敗／回報受阻、Controller 提出的裁定被回答或過期、專案理解投影衝突、Gate 失效需要重驗（後兩者連同組字串的程式一起移除，`save_projection` 不再收 `notice`）。本票不另外補送這些通知；Controller CLI 的事件通知由票 06 的事件等待負責（票 06 涵蓋 Run 結束、受阻、裁定被回答、訊息；投影衝突與 Gate 失效不在其範圍）。`baton_domain::conversation` 整個刪除；但 `DecisionChange.notices`／`ConversationNotice`（Domain 仍會算出、Store 不再寫入）、`workflow::WorkflowNote` 與 `session::SessionOwner::Conversation` 先保留。
- 理由：「不再寫入」適用於所有寫入路徑，否則 conversation_turn 仍會成長。保留的三個 Domain 型別是純邏輯、各有自己的單元測試，票 06 與本票平行實作，可能沿用裁定通知的文字；現在刪掉會擴大與票 06 的衝突面。
- 替代方案：把 Domain 的 `notices` 一併刪除——之後若票 06 要同樣的通知文字得重寫；把投影衝突、Gate 失效改送到事件等待——超出本票，也超出票 06 列的四種事件。
- 影響：投影衝突與 Gate 失效目前只在 UI（知識頁簽、流程頁簽、裁定中心）可見，Controller 不會被通知。票 06 合併後若確定不用 `notices`，應另行刪除這三個型別。
- 審閱：未審

### AI-40：Controller 工具的情境測試改以 Controller CLI 取得憑證，只刪除測舊行為的斷言
- 重要度：低
- 時間／階段：2026-10-23，票 08 實作
- 決策：刪除只測舊功能的測試檔（`conversation.rs`、`controller_handoff.rs`、`controller_handoff_single_account.rs`、`conversation_client.rs`）。其餘以對話喚起 Controller 取得憑證的 harness 測試（Controller 工具、驗收、知識候選、訊息、共用工具、探索、Gate 失效、Workflow、雙軸審查、專案理解、預算、時間線、驗證設定）改用新增的 `Harness::open_controller_cli`／`end_controller_cli`，並為 Controller CLI 建立專用帳號；「這一輪結束後憑證失效」改為「Controller CLI 結束後憑證失效」。只刪除斷言舊行為的部分（對話的工作事件與內容、下一輪 Prompt 帶入的背景、Controller 自動交接）。`methods_playbooks_stages` 的「換帳號後 Workflow 接續」改為使用者以另一個帳號重開 Controller CLI。`timeline_and_metrics` 的「Controller 用量計入 Workspace、不屬於任何 Task」整個刪除：它靠 Controller 輪次的 Adapter 用量回報，Controller CLI 沒有對應的回報。新增 `retired_conversation.rs` 以 harness 驗證端點已移除、工作事件不產生 Controller 輪次、升級前資料在重啟後原樣保留。
- 理由：Controller 工具不變，它們的驗收測試應繼續保護；專用帳號讓票 05（開著 CLI 的帳號不派 Worker）合併後這些測試不必再改。
- 替代方案：直接刪除所有依賴對話的測試——Controller 工具會失去大部分情境覆蓋。
- 影響：harness 測試多一個 Controller 帳號；若有測試斷言帳號分配，日後要注意這個帳號。
- 審閱：未審

### AI-41：Controller 帳號排除放在 Scheduler 的候選條件，交接承接者一併排除
- 重要度：中
- 時間／階段：2026-10-23，票 05 實作
- 決策：`scheduler::Capacity` 新增 `controller_accounts`（有 `starting`／`running` Controller CLI 的 Profile，跨 Workspace），這些帳號不被選為新 Worker 的帳號，等待原因是 `account` 類別、代碼 `controller_account`。一般派工、佇列重試與交接承接者（`successor_placement`）都套用；已在執行的 Worker 照常佔名額、不被停止。交接保留槽的「可用帳號數」仍把 Controller 帳號算進去（與額度受限的帳號相同）。舊的 Controller 一輪不套用（合併時票 08 已把它移除）。同一 Task 續跑時偏好上一個 Run 的帳號，但那個帳號是 Controller 帳號時改派其他帳號。
- 理由：spec「帳號排除」只寫 Scheduler 候選條件；交接的承接者也是 Worker，若不排除，Controller 帳號會在交接時接到工作。沿用 `account` 類別，前端不必新增類別標籤。
- 替代方案：新增 `controller` 等待類別——前端任務看板要多一個標籤；把 Controller 帳號從可用帳號數扣掉——帳號少時會讓交接保留槽忽有忽無。
- 影響：只有一個帳號時，開著 Controller CLI 期間所有新 Task 都會排隊（原因寫明是 Controller 帳號）。
- 審閱：未審

### AI-42：同帳號重開沿用 AI-25 的接續規則，開窗前再確認原生 Session 存在
- 重要度：中
- 時間／階段：2026-10-23，票 05 實作（R019 修正 run 合併票 07 後改寫）
- 決策：要不要帶 `--continue` 沿用 AI-25（這個 Workspace 最近一次真的執行過的 Controller Run 用同一個 Profile；換帳號重開開的新 Run 一律不帶），欄位也沿用 AI-25 的 `continue_session`。本票只多一步：Supervisor 寫 launch intent 前以新的 Port 方法 `AgentAdapter::continue_capability` 查驗，帳號目錄下 `projects\<工作目錄名>\*.jsonl` 確定一個都沒有時，把這個 Run 的 `continue_session` 改為 false，改開新的 Session；讀不到或判斷不了時照原決定接續。結果隨 launch intent 持久化（事件 `controller_cli_launch_started` 的 `continue_session`），Runtime 重建後組出相同的啟動指令。
- 理由：`claude --continue` 找不到 Session 會直接結束（AI-25 的「影響」所說的情形）；上次開啟後沒有任何對話就關掉時，若只看「同帳號」，之後同帳號每次重開都會立刻失敗。
- 替代方案：不查驗——上述情況會卡住；在宿主偵測 `--continue` 失敗後重試——宿主要多一段判斷，違反 shim「只轉送」的定位。
- 影響：查驗依賴 Claude Code 存放 Session 的目錄名規則（與票 15 的 `resume_capability` 相同，2.1.307 實測）；規則變了時會判為「沒有 Session」而開新的，不會卡住。以正式 Adapter 測試同帳號接續的情境要先放一個原生 Session 檔（票 07 的兩個恢復情境測試已補上）。
- 審閱：未審

### AI-43：換帳號重開先記下新 Profile，舊 Run 結束的同一個交易中開新 Run
- 重要度：中
- 時間／階段：2026-10-23，票 05 實作
- 決策：新增 `POST /api/v1/workspaces/{id}/controller-cli/switch`（`{ profile_id }`，202）：在目前的 Run 記下 `end_requested_at` 與 `switch_to_profile_id`（事件 `controller_cli_switch_requested`，含 from／to Profile）；Supervisor 結束宿主後以結束代碼 `switched` 結束舊 Run，同一個交易中開新的 `starting` Run（`switched_from_run_id` 指向舊 Run，存在既有的 `handoff_from_run_id` 欄位），下一次處理開窗。宿主先回報結束（使用者自己關掉）時也照樣開新的。拒絕：沒有開著的（`controller_cli_not_open`）、與目前相同的帳號（`controller_cli_same_profile`）、已在結束或換帳號中（`controller_cli_ending`）、Profile 不可用或不存在。新 Profile 在要求之後才失效時照樣建立新 Run，由開窗前的檢查以 `profile_unavailable` 結束並顯示原因。新增 migration `controller_cli_switch`（只有 `switch_to_profile_id` 一個欄位，排在票 06、07 的 migration 之後；`continue_session` 沿用票 07 的 `controller_cli_recovery`）。換帳號開出的新 Run 第一次等待時依 AI-29 接續上一個 Run 確認到的事件位置；Runtime 重啟後被重新綁定（AI-23）的 Run 也可以換帳號重開，新 Run 取得新的憑證。
- 理由：結束宿主是非同步的（Supervisor 處理）；在同一個交易中開新 Run，頁簽不會看到「舊的已結束、新的還沒開」的空檔，Scheduler 也不會在空檔把帳號派給 Worker；時間線依序是換帳號要求、舊的結束、新的開啟（指回舊的），可追查。
- 替代方案：前端先結束、輪詢到結束再開新的——兩次操作之間 App 關掉就只剩結束；API 同步結束舊 Run——宿主還活著時憑證已失效，`claude` 繼續開著卻不能用工具。
- 影響：`handoff_from_run_id` 對 `kind = 'cli'` 的意思是「換帳號重開的前一個 Run」。
- 審閱：未審

### AI-44：額度與執行中 Worker 數直接取自排程總覽，只在 Controller 開著時輪詢
- 重要度：低
- 時間／階段：2026-10-23，票 05 實作
- 決策：Controller 頁簽不新增 API，以既有的 `GET /api/v1/scheduling`（`accounts[].quota.windows`、`running`、`quota_gate`）顯示「5h 剩 N%｜7d 剩 N%｜執行中 Worker N」（剩餘＝1−較保守的已用比例，最低 0%，未觀測顯示「未知」）；帳號下拉選單與換帳號選單的可用帳號都附上這段，不可用的帳號照舊只顯示狀態。Controller 帳號的 `quota_gate.state` 是 `draining` 時顯示「Controller 帳號的額度已接近上限，建議換帳號重開」。進入頁簽讀一次，Controller 開著時每 2 秒與 Controller 狀態一起重讀；讀不到額度只顯示「無法取得額度」，不擋其他操作。狀態區多一欄「Session」：接續上次的 Session／由換帳號重開開啟的新 Session／新的 Session（合併票 07 時取代它的「原生 Session：接續上次」一欄，見 AI-45）。換帳號選單只列與目前不同、可用的帳號，結束中（含已要求換帳號）時停用。
- 理由：排程總覽已有每個帳號的兩個視窗、門檻與進行中 Run 數，重複做一個端點沒有新資訊；票面的提示門檻寫的是 draining。
- 替代方案：新增 Controller 專用的額度端點——資料與排程總覽重複；`paused` 也提示——paused 是停止派工門檻，對 Controller 自己的互動還有餘裕。
- 影響：Controller 沒開著時，選單上的額度只在進入頁簽時更新（同 AI-22 的帳號清單）。
- 審閱：未審

### AI-45：合併票 06、07 時，`--continue` 只由 Controller CLI 的啟動組裝加一次，狀態區只留一欄 Session
- 重要度：低
- 時間／階段：2026-10-23，票 05 修正（R019，合併 `cli/v1`）
- 決策：兩票都在 Adapter 加了 `--continue`，合併後只保留票 07 在 `controller_spawn_spec` 加的那一處（Worker 不受影響），並把票 07 的 Adapter 測試改為斷言 `--continue` 恰好出現一次；刪除本票在共用組裝中加的那一處與重複的測試。Controller 頁簽狀態區票 07 的「原生 Session：接續上次」與本票的「Session」重複，只留本票那一欄（含換帳號重開的說明），刪除票 07 對應的元件測試（本票的「顯示這次開啟是接續上次的 Session…」測試涵蓋）。
- 理由：同一條規則只能有一份實作（主控在 R015 結果的補充中指定）；兩欄顯示同一件事會讓使用者困惑。
- 替代方案：保留兩欄——資訊重複；保留票 07 的一欄——看不出這次是不是換帳號重開開的新 Session。
- 影響：無行為改變；`--continue` 的判斷與欄位都沿用票 07。
- 審閱：未審

### AI-46：Windows 平台手動驗收清單獨立成 docs/windows-acceptance.md
- 重要度：低
- 時間／階段：2026-10-23，票 09 實作
- 決策：新建 `docs/windows-acceptance.md` 當作 repo 內的 Windows 平台手動驗收清單，把票 03 放在 `docs/validation.md` 檔尾的「Windows 平台手動驗收：Controller CLI 開窗（票 03）」整節搬過去（原 7 項成為 A1～A7，`controller_terminal` 的自動化說明成為 F 節），再依票面補上 A8～A14（主控台主機對照、原生互動、操作手冊、Controller 工具）、B 事件等待、C 帳號排除、D 完全退出、E 以既有目錄建立 Profile；每項一列「步驟／預期結果」，開頭附執行紀錄範本。`docs/development.md` 的 UI E2E 一節加上連結。
- 理由：AI-21 指定票 09 把這一節搬到 Windows 驗收清單；原清單在 GitHub issue #3 §15，Worker 不能 push 也不改外部 issue。`docs/validation.md` 其餘內容是 issue-orchestrator skill 的英文驗證紀錄，與桌面 App 的實機驗收不同性質。
- 替代方案：留在 `docs/validation.md` 擴充——兩種性質的紀錄混在同一檔；寫回 GitHub issue #3——超出 Worker 權限。
- 影響：AI-15 的「影響」仍寫實機行為列在 `docs/validation.md`，現在應讀作 `docs/windows-acceptance.md` 的 A1、A8。platform-rebuild-v1 票 51 的實機驗收可一併使用這份清單。
- 審閱：未審

### AI-47：Controller CLI 的 UI E2E 以 API 備妥前置，driver 新增完全比對的文字定位
- 重要度：低
- 時間／階段：2026-10-23，票 09 實作
- 決策：新旅程 `crates/baton-harness/tests/ui_e2e/controller_cli_open_and_end.rs`：第二個帳號（「協調帳號」）與兩個 Workspace 以 API 建立後重新載入頁面，之後全部經畫面——側欄「選擇 beta」→「Controller」頁簽→在「Controller 帳號」選單選「協調帳號（5h 未知｜7d 未知｜執行中 Worker 0）」→「開啟」（開啟中）→`supervise` 後等頁簽自行重讀到「執行中」→「結束」（結束中…）→`supervise` 後「已結束」。佐證只讀：fake Claude 環境 Port 只開了一個視窗、`CLAUDE_CONFIG_DIR` 是選的帳號、工作目錄是該 Workspace；宿主在 fake ProcessControl 被停止且不再存活；API 的 Run 是 `ended`。driver 新增 `exactText`（`browser::exact_text`），因為「執行中」會以子字串比對到「執行中 Worker 0」。不遺留暫存目錄與程序沿用票 53 的機制（`scripts/test-e2e.mjs` 的專用 TEMP 檢查、`finish` 斷言 Chromium 與 driver 已結束），Controller 宿主是 fake 程序，以上述「不再存活」斷言。
- 理由：票面的旅程從「選 Workspace」開始，Workspace 與帳號的建立已由其他旅程與 smoke 涵蓋；兩個帳號才能驗證「選 Profile」確實改變了開窗的帳號（預設會選第一個）。
- 替代方案：在畫面建立 Workspace 與帳號——帳號登入要靠 fake 終端機登入，與本旅程無關且拉長測試；以 `has_text` 過濾——仍無法排除「執行中 Worker 0」。
- 影響：選項文字含額度摘要，Controller 頁簽的選項格式（AI-44）改變時要一起改這個測試。
- 審閱：未審

### AI-48：「CLI 結束後 wait 失效（結束碼 3）」的實機驗收改以 Runtime 重啟觸發；設定目錄要用完整路徑
- 重要度：中
- 時間／階段：2026-10-23，票 09 實作
- 決策：Windows 驗收清單 B5／B6 以「在工作管理員結束 `baton-runtime.exe`、等監護重新啟動」驗證憑證失效：掛著的 wait 以結束碼 1 結束，Runtime 重新綁定（AI-23）後再執行 wait 以結束碼 3 結束並印出「Controller 憑證已失效…」。不寫成「關掉 Controller CLI 後 wait 以 3 結束」。E 節的範例路徑用 `C:\Users\<使用者>\.claude-Work-1`，另列一項驗證 `%USERPROFILE%`／`~` 被拒。
- 理由：宿主把 `claude` 與它的子程序放進「宿主結束時一併終止」的 Job（AI-19），關掉 CLI 時背景 wait 會跟著被終止，實機上看不到結束碼 3；只有 `claude` 還開著而憑證失效（重新綁定）時才看得到。平台不展開 `~` 與環境變數（AI-07），照票面字面輸入 `~/.claude-Work-n` 會被拒。
- 替代方案：照 spec 故事 48 的字面寫「CLI 結束後 wait 以 3 結束」——驗收者在實機上無法重現，會誤判為失敗。
- 影響：spec 故事 48 的「CLI 結束後等待指令自動失效」在實機上主要是「隨程序樹一起被終止」，結束碼 3 只在重新綁定後出現；若使用者希望關掉 CLI 時 wait 以 3 結束，需要改宿主的 Job 設定。
- 審閱：未審

### AI-49：最終 code-review 分三個範圍審查、兩個範圍修正
- 重要度：中
- 時間／階段：2026-10-23，最終 code-review
- 決策：`cli/v1` 相對 `rebuild/v1`（不含 `.scratch`）120 個檔案、約 1.6 萬行。照 platform-rebuild-v1 的 AI-448，在唯讀 worktree 上分三個範圍各派一個審查 run（R026 Domain／Store、R027 Runtime／API／Ports／shim／Client、R028 Web／Tauri／文件／harness），共回報中 3、低 13，沒有高嚴重度；加上 `review-notes.md` 尚未處理的 9 項。修正改分兩個範圍平行處理：final-ab（R026＋R027 與後端的待修項）、final-c（R028 與 harness／前端的待修項），各自驗證後合併。
- 理由：R026 的換帳號與 `begin_launch` 時序修正需要同時改 Store 的 `end_cli` 與 Supervisor 的呼叫端，和 R027 的範圍重疊；分三個修正 run 必然改到同一批檔案。項目數量（後端約 14 項、前端與 harness 約 10 項）兩個 run 處理得完。implement-spec skill 要求單一修正 run，但 harness 不穩定測試的重構橫跨十多個測試檔，與後端修正放在同一個 run 會讓單一 context 負擔過重。
- 替代方案：照 skill 字面派單一修正 run（範圍跨整個系統，驗證也難在一個 context 完成）；照 AI-448 分三個修正 run（A、B 必然衝突）。
- 影響：兩個修正 run 可能都動到 `crates/baton-harness`（後端修正補的情境測試）與 `docs/`，後合併的需要整合 run。
- 審閱：未審

### AI-50：換帳號後要不要開新 Run 由 Store 依交易內的 Run 決定，重啟後中斷也會開
- 重要度：中
- 時間／階段：2026-10-23，最終 review 修正（R029-fix-ab）
- 決策：Store 的 Controller CLI 寫入統一走 `apply_cli(run_id, decide, next, now)`：Domain 判定後 Run 進入終態，而交易內讀到的 Run 有 `switch_to_profile_id` 時，同一個交易中以 `next` 開新的 Run。Supervisor 的結束、`begin_launch`、重啟後的中斷一律備妥新 Run 的識別（取不到亂數才傳 `None`），不再依這一輪開始時的快照判斷；完全退出仍傳 `None`，不開新 Run。開窗前「已按結束／換帳號重開」改由 Domain 的 `begin_launch` 判斷，以 `user_ended`／`switched` 結束、不寫 launch intent，Supervisor 依快照的檢查移除。Runtime 在換帳號要求之後當掉、重啟時宿主已不在：舊 Run 照 AI-24 標為 `interrupted`（`runtime_restarted`），同一個交易中以新帳號開新的 Run。
- 理由：R026 第 1、2 項與 review-notes 第 10 項都是「依舊快照決定」造成的缺口。規則放在交易內，就不必顧慮呼叫端讀快照的時間點。AI-43 已規定宿主先結束時照樣開新的，中斷也是同一種情況。
- 替代方案：Supervisor 寫入前重讀一次 Run——兩次讀取之間仍有空檔；當掉後改以 `switched` 結束舊 Run——要多一條 Domain 規則，而且宿主並不是被 Runtime 結束的。
- 影響：換帳號後經過當掉，時間線依序是換帳號要求、`controller_cli_interrupted`、新 Run 的 `controller_cli_opened`。測試：`controller_cli_accounts.rs` 的 `a_switch_requested_before_a_crash_*`、`a_switch_requested_before_the_window_opened_*`，以及 Domain 的 `a_run_whose_end_was_requested_before_the_launch_intent_*`。
- 審閱：未審

### AI-51：從備份還原時，尚未結束的 Controller CLI 一律中斷
- 重要度：低
- 時間／階段：2026-10-23，最終 review 修正（R029-fix-ab）
- 決策：`end_unfinished_runs` 在同一個交易中把非終態的 `kind = 'cli'` Run 以 Domain 的 `interrupt` 中斷，原因與 Worker 相同，都是 `RESTORE_INTERRUPT_REASON`（`restored_from_backup：…`），結束代碼沿用 `runtime_restarted`。不開新 Run，也不處理換帳號要求。數量計入還原報告的 `interrupted_runs`。
- 理由：R026 第 3 項：備份當下剛要求開啟的 CLI，還原後會被自動開窗。這是還原的處理，不是 migration，不違反 AI-38。沿用 `interrupt` 就不用新增結束代碼，頁簽顯示的是結束原因。
- 替代方案：新增 `restored_from_backup` 結束代碼——前端要多一個對照，資訊和原因文字重複；只處理 `starting`——`running` 的列要等 AI-24 查驗才收尾，而備份裡的宿主身分在還原後的機器上沒有意義。
- 影響：還原演練的 `interrupted_runs` 現在也包含 Controller CLI。
- 審閱：未審

### AI-52：Controller 憑證與 Controller 寄件者共用 Domain 的 credential_valid
- 重要度：低
- 時間／階段：2026-10-23，最終 review 修正（R029-fix-ab）
- 決策：Store 新增 `controller_credential_valid(conn, scope)`：以 `kind = 'cli'` 載入 Run，再交給 Domain 的 `credential_valid` 判斷（尚未結束、沒有重新綁定）。`controller_run_active` 與 `messages.rs` 的 Controller 寄件者 `is_current` 都改用它，SQL 版本的規則刪除。升級前的舊輪次（`kind = 'turn'`）當寄件者時回 `RunNotFound`，不再依狀態判斷。沒有呼叫端的 `Store::controller_cli_accounts` 一併刪除（review-notes 第 9 項），Scheduler 與交接直接用 `load_controller_accounts`。
- 理由：R026 第 4 項：規則原本有三份，而且不一致。保留 Domain 那一份，它有單元測試。
- 替代方案：刪除 Domain 的 `credential_valid`，在 `messages.rs` 複製 SQL 條件——規則仍是兩份。
- 影響：API 本來就先以 `controller_run_active` 擋掉舊輪次與重新綁定的憑證，所以對外行為不變。
- 審閱：未審

### AI-53：結束中重新開啟回 Ending，換帳號遇到結束中改用 SwitchWhileEnding（同一個錯誤代碼）
- 重要度：低
- 時間／階段：2026-10-23，最終 review 修正（R029-fix-ab）
- 決策：`open` 遇到 `end_requested_at` 有值的非終態 Run 時，回 `Ending`（「正在結束，請等它結束後再開啟」），不回 `AlreadyOpen`。`request_switch` 遇到結束中改回新的 `SwitchWhileEnding`（「正在結束或換帳號重開中，不能再換帳號；請等它完成」）。兩者的錯誤代碼都是 `controller_cli_ending`。
- 理由：R026 第 5 項。錯誤代碼不變，前端與 `baton-client` 的既有判斷都不用改。
- 替代方案：只改 `Ending` 的訊息，讓兩種情境共用——一句話很難同時說清「再開啟」與「不能再換」。
- 影響：只有訊息文字改變。
- 審閱：未審

### AI-54：AgentAdapter 新增 prepare_controller，開啟 Controller CLI 只寫 MCP 設定
- 重要度：低
- 時間／階段：2026-10-23，最終 review 修正（R029-fix-ab）
- 決策：Port 新增 `AgentAdapter::prepare_controller`，預設不做任何事，fake 不必改。`ClaudeCodeAdapter` 的實作只在 `runs\<id>\` 寫 `mcp.json`，並以 Controller 宿主的引數（跳過 `controller-host <cmd.exe> /d /v:off /c call`）做批次檔安全檢查。Supervisor 開啟 Controller CLI 時改呼叫它。
- 理由：review-notes 第 6 項：沿用 Worker 的 `prepare` 會多寫出用不到的 `prompt.md` 與 Worker 通則 `system-prompt.md`。Controller CLI 的系統提示是唯讀目錄裡的操作手冊，`--mcp-config` 仍指向 Run 目錄的 `mcp.json`。
- 替代方案：在 `RunSpec` 加一個旗標，讓 `prepare` 分辨兩種 Run——同一個方法有兩套語意，而且 Worker 的呼叫端也要跟著改。
- 影響：日後其他 Adapter 若要支援 Controller CLI，要自己實作 `prepare_controller`。
- 審閱：未審

### AI-55：完全退出時逐筆收集 Controller CLI 的停止結果，但不提前到 Worker 找回程序之前開始
- 重要度：低
- 時間／階段：2026-10-23，最終 review 修正（R029-fix-ab）
- 決策：`stop_controller_clis_for_exit` 改為每個 CLI 一個 future（`FuturesUnordered`），與 Worker 的停止合成同一個串流，完成一筆就記一筆。總上限到了仍沒有結果的，才在 `finish_exit` 記為 `stop_unconfirmed`；Controller CLI 的訊息改為「Run 維持原狀」，不再寫「保留寫入所有權」。CLI 的停止仍在 Worker 的 find 迴圈之後才開始執行。
- 理由：R027 第 2 項的主要缺陷是一個 CLI 停不下來，整批結果就被丟掉，報告與 Run 狀態不一致，這部分已修正。提前開始則要讓 CLI 的停止與 find 迴圈並行，兩邊都要寫入停止報告，必須重排整段停止流程。find 迴圈一般很快，而且受同一個總上限約束，AI-26 的「同時停止」實際上只差找回程序的時間。
- 替代方案：把 CLI 的停止 spawn 成獨立的 task 並提前開始——`Supervisor` 以參照借用，要改成 `Arc` 或重排停止流程，風險比收益大。
- 影響：Worker 很多、find 很慢時，Controller CLI 的停止會晚一點開始，仍在總上限內。
- 審閱：未審

### AI-56：Controller 工具的說明與錯誤訊息改寫成 Controller CLI 的模型，不改行為
- 重要度：中
- 時間／階段：2026-10-23，最終 review 修正（R029-fix-ab）
- 決策：MCP shim 的工具說明（`propose_decision_request`、`read_inbox`）與 Agent API 回給 Controller 的訊息要改寫：
  - 範圍：必須送裁定、已建立裁定請求、雙軸審查與設計偏離的必須送裁定、Controller 憑證失效的 401。
  - 「使用者回答後你會在對話中收到工作事件」改為「使用者回答後，背景執行的 wait 會回報「裁定已回答」事件」。
  - `read_inbox` 的「建議每一輪開始時呼叫」改為「建議每次收到事件時呼叫」。
  - 401 訊息改為「Controller CLI 已結束或已重新綁定，憑證已失效」。
  - 工具名稱、參數、結構、錯誤碼與行為都不變。
- 理由：R027 第 1 項：票 08 之後已經沒有對話的工作事件，舊文字會讓 Controller CLI 的模型以為不必掛背景 wait。spec 的「MCP shim 不變」指的是介面與行為，說明文字跟著實際的通知機制修正，並不違反。
- 替代方案：完全不動 shim，只在操作手冊補說明——工具說明與手冊互相矛盾。
- 影響：只影響送進模型的文字；「裁定已回答」與 `wait` 印出的事件標籤一致。其他 crate 的註解（不送進模型）仍有零星的「一輪」用語，未全面改寫。
- 審閱：未審

### AI-57：刪除對話退役後沒有讀者的 Domain 型別，SessionOwner 只刪對話變體
- 重要度：低
- 時間／階段：2026-10-23，最終 review 修正（R029-fix-ab）
- 決策：依 AI-39 的後續清理（review-notes 第 7 項）：
  - 刪除 `DecisionChange.notices`／`ConversationNotice`，連同組字串的程式與「答案推翻假設」的通知。
  - 刪除 `workflow::WorkflowNote` 與 `SessionOwner::Conversation`。
  - 單元測試只刪掉斷言這些輸出的部分。
  - `decision.rs` 的「喚醒 Controller」註解改寫。
  - 保留 `AgentRole::Controller` 與 `AgentSession.conversation_id`；`SessionOwner` 只剩 `Task`，但 enum 與 `*_for` 函式先保留。
- 理由：票 06 已合併且沒有沿用這些輸出，整個 workspace 都沒有讀者。`SessionOwner` 不會序列化；資料庫存的 `role` 與 `conversation_id` 仍保留，讀取升級前的資料不受影響，符合 AI-38。
- 替代方案：把 `SessionOwner` 整個收掉——會多動 run.rs 與測試，不屬於這次清理。
- 影響：與 AI-39 之後的實際行為相同，只是把產生了卻沒人讀的資料移除。
- 審閱：未審

## Spec 決策審閱紀錄

（使用者審閱 grilling.md 的 Q1～Q20 時記在這裡。）
