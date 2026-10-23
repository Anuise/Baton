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

## Spec 決策審閱紀錄

（使用者審閱 grilling.md 的 Q1～Q20 時記在這裡。）
