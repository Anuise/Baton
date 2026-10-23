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

### AI-23：固定指示與共同指示合併成一個系統提示檔，Worker 的 `--append-system-prompt-file` 改指向 Run 目錄
- 重要度：中
- 時間／階段：2026-10-23，票 04 實作
- 決策：Claude Code 只帶一個 `--append-system-prompt-file`，內容是「固定指示＋（有的話）`---` 分隔＋共同設定的 `CLAUDE.md`」（`baton_domain::playbook::system_prompt`）。Worker：Adapter 的 `prepare` 寫 `runs\<run_id>\system-prompt.md`（Worker 通則＋快照的共同指示），`spawn_spec` 一律帶這個檔案，即使沒有共同設定。Controller CLI：Supervisor 以 `write_readonly_files` 把操作手冊（＋共同指示）寫成 `run-settings\<run_id>\system-prompt.md`，與設定快照同一個唯讀目錄；這個目錄現在一定存在，沒有共同設定版本時只放操作手冊。原本斷言 Worker 的 `--append-system-prompt-file` 指向快照 `CLAUDE.md`（或沒有共同設定時不帶這個旗標）的 4 處既有測試，改為斷言指向合併檔並檢查檔案內容含共同指示；其餘 Worker 啟動測試沒有改。
- 理由：`--append-system-prompt-file` 在 `claude --help` 中沒有列出，重複指定時是否都生效無從確認；同時用 `--append-system-prompt` 帶內嵌文字也可能被拒，而且經 cmd.exe 啟動 `claude.cmd` 時，換行與特殊字元會被 `prepare` 擋下。合併成一個檔案的行為最確定。
- 替代方案：重複指定兩個 `--append-system-prompt-file`——若只有最後一個生效，共同指示或通則就會悄悄消失；把通則放進 Prompt（標準輸入）——不是票面要求的系統提示。
- 影響：舊的 Controller 輪次（`claude -p`，票 08 會移除）也走 `spawn_spec`，在移除前同樣會帶 Worker 通則。升級前已在執行的 Worker 組出的啟動指令與現在不同（多一個旗標），只有在不完全退出就換版本時才會影響以 command_id 找回程序。
- 審閱：未審

### AI-24：操作手冊內建在 domain、版本號為 "1"，頁簽顯示 App 內建的版本
- 重要度：低
- 時間／階段：2026-10-23，票 04 實作
- 決策：操作手冊、Worker 通則與 AI 決策紀錄範本是 `crates/baton-domain/src/playbook/*.md`，以 `include_str!` 編進程式，並用該目錄的 `.gitattributes` 固定為 LF（repo 的 `core.autocrlf=true` 會讓不同 checkout 編進不同的換行）。`PLAYBOOK_VERSION = "1"`，手冊標題行是「# Baton Controller 操作手冊（版本 1）」，以測試確保兩者一致。`ControllerCliView` 新增 `playbook_version`（從未開啟過也有），Controller 頁簽的狀態欄列出「操作手冊版本」。版本號不記在每個 Run 上。「不含帳號名稱、額度數字、並行上限、worktree 路徑」以測試檢查：禁用字串（`Work-`、`.claude-`、`%`、`5h`、`7d`、`≥`、`-wt`、`../`、磁碟路徑等），並規定內文除了編號清單的序號，唯一的數字只能是「最多修正 3 次」。
- 理由：Run 的手冊檔是開啟時由同一個執行檔寫入的，版本必定等於內建版本；只有「Controller 開著時更新 App」才會不同，而更新要先完全退出，Controller CLI 也會一起結束（票 07）。
- 替代方案：在 `controller_run` 記錄開啟時的手冊版本——要加 migration，目前沒有使用情境。
- 影響：修改手冊內容時要記得調整 `PLAYBOOK_VERSION` 和標題（標題那一半有測試會擋）。
- 審閱：未審

### AI-25：Worker 通則不寫死 AI 決策紀錄的路徑，由 Controller 寫進 Task 契約
- 重要度：中
- 時間／階段：2026-10-23，票 04 實作
- 決策：Worker 的工作目錄是 Task 的 worktree，不是 Workspace 目錄，而 Worker 通則是固定文字，不知道 Workspace 在哪裡。所以通則寫「路徑寫在 Task 契約中；沒寫時在最後的回報中列出你的決定，由 Controller 代為追加」；操作手冊要求 Controller 派工時在契約的輸入寫明 `<Workspace>\.baton\ai-decisions.md` 的完整路徑，並代為追加 Worker 只在回報中列出的決定。
- 理由：spec 要求通則固定，又要求 Worker 把決定寫進 Workspace 的紀錄檔；由 Controller 補上路徑，兩者都能滿足。
- 替代方案：Adapter 依 Run 的 Workspace 把路徑插進系統提示——通則就不再固定，Adapter 也要多知道 Workspace 目錄；寫相對路徑 `.baton/ai-decisions.md`——在 worktree 裡會寫到錯的檔案（或寫進分支裡）。
- 影響：Worker 寫 Workspace 目錄要經過權限判定（Workspace 授權不允許時會被擋）；被擋時就照通則改在回報中列出。
- 審閱：未審

### AI-26：AI 決策紀錄的建立方式、範本與失敗處理
- 重要度：低
- 時間／階段：2026-10-23，票 04 實作
- 決策：Supervisor 寫下 launch intent、確認找得到 `claude` 之後、準備唯讀目錄之前，以新的 `WorkspaceFs::create_file_if_absent`（`create_new` 開檔，需要時建立 `.baton` 目錄）建立 `<Workspace>\.baton\ai-decisions.md`：已存在（包括檢查之後才被建立的）一律不動。建立失敗（例如 `.baton` 是一個檔案）時以 `launch_failed`、原因「無法建立 AI 決策紀錄 …」結束這次開啟，不開窗。範本照 `.scratch/platform-rebuild-v1/ai-decisions.md` 的檔頭：標題改為「# AI 決策紀錄」，說明改成 Workspace 適用的文字，拿掉「spec 決策記在 spec.md 的 D-01～D-33」那一行；重要度、審閱行規則、條目格式區塊與 `---` 逐字相同（有測試）。
- 理由：沒有紀錄檔時，手冊要求 Controller 追加的對象就不存在；直接以失敗呈現，使用者看得到原因。用 `create_new` 而不是「先讀再原子改名」，是因為改名會覆蓋檢查之後才出現的檔案。
- 替代方案：建立失敗時只記錄、照常開窗——Controller 之後追加也很可能失敗，問題會被延後、更難發現。
- 影響：寫到一半時當掉，可能留下內容不完整的範本檔，之後也不會被覆寫（檔案很小，機率極低）。
- 審閱：未審

### AI-27：操作手冊裡等待指令、受阻與決策權限表的寫法
- 重要度：中
- 時間／階段：2026-10-23，票 04 實作
- 決策：等待指令寫成「MCP shim 的 `wait` 子命令，以 Bash 工具的背景方式執行」，不寫執行檔路徑與參數（票 06 才會定下來）。Controller 工具沒有「把 Task 標為受阻」的專用工具，所以修正 3 次仍不通過時，手冊寫成「以 `update_task` 在契約的阻礙處理記下原因，不再派工」。決策權限表列出 D-15 中必須送裁定的事項，但不列「三次恢復失敗後的重試」（只能由使用者從恢復介面發起，Controller 不能提出）；不確定時先呼叫 `controller_action` 讓 Runtime 判定。驗證要「由新的 Worker Run 執行、不沿用實作的 Session」，因為帳號由 Runtime 選，Controller 無法指定不同的帳號。
- 理由：手冊不能寫還不存在的介面細節，也不能要求 Controller 用平台沒有的工具。
- 替代方案：先寫出猜測的 `wait` 指令列——票 06 若定得不同，Controller 就會執行錯的指令。
- 影響：票 06 確定 `wait` 的呼叫方式（執行檔位置、參數、結束碼）後，應更新手冊並把 `PLAYBOOK_VERSION` 升到 "2"。之後若新增「標記受阻」的 Controller 工具，也要改手冊。
- 審閱：未審

## Spec 決策審閱紀錄

（使用者審閱 grilling.md 的 Q1～Q20 時記在這裡。）
