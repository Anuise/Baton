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

### AI-06：全畫面首次精靈的條件、內容與「先進入主畫面」
- 重要度：中
- 時間／階段：2026-10-23，票 01 實作
- 決策：App 以 `GET setup` 判斷，沒有任何「使用中且可派工（`dispatchable`）」的 Account Profile 時，以全畫面顯示首次精靈＋帳號頁（沒有頁簽與側欄）；讀不到首次設定狀態時照常顯示頁簽。全畫面下方有「先進入主畫面」，只在這次開啟有效。有可派工 Profile 後，首次精靈改放在「帳號」頁簽最上方。
- 理由：`dispatchable` 只反映登入與身分核對（不含額度），最貼近 spec「可用 Account Profile」；精靈的文字要使用者在「下方帳號」新增與核對 Profile，全畫面若沒有帳號頁就無法完成設定。所有帳號登入暫時失效時若沒有出口，使用者連 Workspace 與裁定都看不到；冒煙腳本（`scripts/smoke-desktop.mjs`）的 Profile 也從未登入，需要這個出口才能走到 Workspace 與設定頁簽。
- 替代方案：改成「沒有任何 Profile」才全畫面——與 spec 的「可用」不符；維持無出口——冒煙腳本的平台狀態、授權與恢復三段失去 UI 覆蓋，且登入失效時整個 App 被鎖住。
- 影響：多一顆票面沒提到的按鈕；若使用者不要這個出口，移除按鈕並把冒煙腳本改成以 API 檢查那三段。
- 審閱：未審

### AI-07：「新選 Workspace 預設 Controller」的解讀
- 重要度：低
- 時間／階段：2026-10-23，票 01 實作
- 決策：從「沒有選中任何 Workspace」到選中一個（包括在側欄建立後自動選中）時，功能頁簽重設為 Controller；在兩個 Workspace 之間切換時停在同一頁簽。建立 Workspace 後因此不再直接進入授權設定，UI E2E 與冒煙腳本改為建立後點「授權」頁簽。
- 理由：票面同時要求「切換時停在同一頁簽」與「新選 Workspace 預設 Controller」，只有把「新選」解讀為「先前沒有選中」兩者才不衝突。
- 替代方案：建立新 Workspace 時一律回 Controller（或授權）——要另外區分「建立」與「切換」兩種選取，規則較難預期。
- 影響：使用者建立 Workspace 後要自己切到「授權」頁簽確認授權；票 03 填好 Controller 頁簽後可再評估是否在那裡提示「尚未確認授權」。
- 審閱：未審

### AI-08：頁簽配置細節
- 重要度：低
- 時間／階段：2026-10-23，票 01 實作
- 決策：「知識」頁簽把知識與專案理解上下疊放、「授權」頁簽把授權設定與模型設定上下疊放；只有「審查與驗證」用小頁簽（不記住所選的小頁簽）。左側 Workspace 清單只在 `Workspace` 頂層頁簽顯示。所在位置以 localStorage 的 `baton.shell` 保存頂層頁簽、Workspace id 與功能頁簽；上次的 Workspace 已不在清單（例如已封存）時顯示「請從左側選擇或建立 Workspace」。「裁定」頁簽的名稱固定為「裁定」，待裁定數以數字徽章顯示、title 為「待裁定 N」。
- 理由：票面只對「審查與驗證」指定小頁簽，知識與授權寫的是「＋」；舊畫面的授權與模型設定本來就疊在一起。頁簽名稱固定，E2E 才能以完全比對點選，也不會與側欄的「待裁定 N」文字重複比對。
- 替代方案：知識與授權也用小頁簽——多一層點選；側欄常駐於三個頂層頁簽——帳號與設定頁簽會出現與其無關的 Workspace 清單。
- 影響：知識頁簽較長；之後若要改成小頁簽，只動 `App.tsx` 的 `WorkspaceView`。
- 審閱：未審

## Spec 決策審閱紀錄

（使用者審閱 grilling.md 的 Q1～Q20 時記在這裡。）
