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

## Spec 決策審閱紀錄

（使用者審閱 grilling.md 的 Q1～Q20 時記在這裡。）
