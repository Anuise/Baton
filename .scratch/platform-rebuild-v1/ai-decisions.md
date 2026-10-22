# AI 決策紀錄：platform-rebuild-v1

本檔收錄 AI 在拆票與實作期間代為決定的事項。使用者會依序查看，再插入修正。

- spec 階段的產品與架構決策記在 [spec.md](spec.md) 的「Claude 決策索引」（D-01～D-33），本檔不重複。
- 本檔記錄拆票、執行方式與實作期間的決策，編號為 AI-NN，只能往後追加。
- **重要度**：
  - **高**：做錯了會讓帳號額度、安全、資料或大量工作白費，請優先查看。
  - **中**：影響工作方式或範圍，事後修改有一定成本。
  - **低**：預設值或慣例，改起來便宜。
- 每個條目都有一行 `- 審閱：未審｜認同 YYYY-MM-DD｜已修正`。新條目一律寫 `- 審閱：未審`；找未審條目用 `grep -n "^- 審閱：未審"`。
- 使用者表示同意時，把審閱行改成 `- 審閱：認同 YYYY-MM-DD`。
- 使用者修正時，把審閱行改成 `- 審閱：已修正`，並照舊在條目下方加一行 `使用者修正：…`；AI 看到後依修正調整，並在需要時另開新條目說明影響。

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

### AI-01：「5h limit >= 3、7d >= 3」的解讀
- 重要度：高
- 時間／階段：2026-10-22，拆票
- 決策：解讀為「剩餘百分比 ≥ 3」，也就是 `/usage` 顯示的已用量 ≤ 97% 才可以派出。
- 理由：`/usage` 只提供百分比；若解讀為已用量 ≥ 3%，剛重置的帳號反而不能用，不合理。
- 替代方案：解讀為「剩餘量足夠跑 3 個 run」。這需要每個 run 的用量估計，目前沒有資料。
- 影響：門檻非常寬鬆。帳號在 run 執行中撞牆的機率較高，要靠 watchdog 等待（AI-14）。
- 審閱：認同 2026-10-22

### AI-02：清除繼承的環境變數，避免用到主帳號
- 重要度：高
- 時間／階段：2026-10-22，拆票
- 決策：啟動 `claude -p` 前，移除從主控 session 繼承的 `CLAUDE*`、`CLAUDECODE`、`ANTHROPIC*`、`AI_AGENT`、`BAGGAGE` 環境變數，只留下 `CLAUDE_CONFIG_DIR` 與 watchdog 變數。
- 理由：主控 session 的環境帶有主帳號的身分與 host 認證相關變數，直接繼承可能讓子程序沿用主帳號。實測清除後，Work-3 的 `/usage` 顯示的是 Work-3 自己的用量。
- 替代方案：只覆寫 `CLAUDE_CONFIG_DIR`。這無法排除其他認證來源的影響。
- 影響：`ANTHROPIC_API_KEY` 這類變數若是使用者刻意設定的，也會一併被清掉。
- 審閱：認同 2026-10-22

### AI-03：可派工的帳號清單來源
- 重要度：低
- 時間／階段：2026-10-22，拆票
- 決策：每次派工前列出 `.claude-Work-*` 目錄並排除 Work-0。盤點當時，Work-1 已達週上限（10/26 22:00 重置），實際可用的是 Work-3、Work-8、Work-9。
- 理由：帳號可能增減，寫死名單容易過期。
- 替代方案：在 execution.md 寫死名單。
- 影響：新增的 Work-n 只要完成登入，就會自動被使用。
- 審閱：未審

### AI-04：主控 session 不使用內建 subagent
- 重要度：中
- 時間／階段：2026-10-22，拆票
- 決策：實作、驗證、審查與程式碼探索，全部經由 Work-n 的 `claude -p` 執行；主控只做排程、記帳與輕量讀檔。
- 理由：內建 subagent 用的是主帳號額度，使用者要求不要用到主帳號的流量。
- 替代方案：允許主控用 subagent 做唯讀探索。
- 影響：主控本身仍會消耗少量主帳號額度，這無法避免。
- 審閱：未審

### AI-05：`/usage` 無法解析時的處理
- 重要度：低
- 時間／階段：2026-10-22，拆票
- 決策：輸出含有撞牆訊息、或解析不到兩個百分比時，視為不可用，改用下一個帳號。
- 理由：寧可少派一個，也不要把 run 派到狀態不明的帳號。
- 替代方案：直接派出，靠 watchdog 等待。
- 影響：`/usage` 的輸出格式改版時，所有帳號都會被判為不可用，主控必須回報並暫停。
- 審閱：未審

### AI-06：Git Bash 執行 `/usage` 要關閉路徑轉換
- 重要度：中
- 時間／階段：2026-10-22，拆票
- 決策：查詢額度時一律設定 `MSYS_NO_PATHCONV=1`。
- 理由：實測未設定時，`/usage` 被轉成 `C:/Program Files/Git/usage` 並送給模型，Work-3、Work-8、Work-9 各被消耗了一次小額回應。
- 替代方案：改用 PowerShell 執行查詢。
- 影響：無。
- 審閱：認同 2026-10-22

### AI-07：watchdog 變數的實際名稱與數值
- 重要度：中
- 時間／階段：2026-10-22，拆票
- 決策：設定 `CLAUDE_CODE_RETRY_WATCHDOG=1`，並以 `CLAUDE_CODE_RETRY_WATCHDOG_MAX_USAGE_LIMIT_WAIT_MS=18000000` 把撞到用量上限時的最長等待設為 5 小時。
- 理由：Claude Code 2.1.307 的執行檔中有這兩個變數；後者專門控制用量上限的等待時間，內部預設約 6 小時。這是從執行檔字串推斷的，沒有官方文件。
- 替代方案：另外設定 `CLAUDE_CODE_RETRY_WATCHDOG_MAX_WAIT_MS`。它會限制所有重試的總等待時間，範圍太廣，所以不設定。
- 影響：若 CLI 改版改了變數名稱，watchdog 會靜默失效，run 撞牆後就會直接失敗。
- 審閱：未審

### AI-08：子代理的權限模式
- 重要度：高
- 時間／階段：2026-10-22，拆票
- 決策：`--permission-mode bypassPermissions`，每個 run 只在該票自己的 worktree 中執行。
- 理由：`-p` 模式沒有人可以回答權限提示；這些票需要執行 cargo、pnpm、git 與測試。這也和 spec 的 DR-20261021-02「可信本機執行」一致。
- 替代方案：`acceptEdits` 加上逐項列出允許的工具。這較安全，但很可能因為漏列某個工具而讓 run 卡住或失敗。
- 影響：子代理以使用者的身分執行任意命令，worktree 不構成安全邊界。
- 審閱：認同 2026-10-22

### AI-09：run 的輸出格式
- 重要度：低
- 時間／階段：2026-10-22，拆票
- 決策：使用 `--output-format stream-json --verbose`，完整寫入紀錄檔。
- 理由：之後可以從紀錄檔查出限流、費用與原生 Session 識別，也和 spec 的 Adapter 設計（D-06、D-07）相同，可以順便累積實測資料。
- 替代方案：純文字輸出。
- 影響：紀錄檔較大。
- 審閱：未審

### AI-10：每張票一個 worktree 與分支，並合併到整合分支
- 重要度：中
- 時間／階段：2026-10-22，拆票
- 決策：每張票從整合分支 `rebuild/v1` 建立 worktree 與分支 `rebuild/NN-<slug>`；驗證通過後合併回 `rebuild/v1`。`main` 在第 52 張票之前都不動。
- 理由：最多 3 個帳號 × 3 個 run 同時寫入，共用 checkout 會互相衝突。
- 替代方案：直接合併到 `main`。
- 影響：重建期間 `main` 仍保持舊版 skill 的樣子。
- 審閱：未審

### AI-11：不使用 issue-orchestrator skill 本身來執行
- 重要度：高
- 時間／階段：2026-10-22，拆票
- 決策：由主控 session 依 execution.md 的流程直接調度，不呼叫本 repo 現有的 issue-orchestrator skill。
- 理由：現有 skill 的 v1 限定單一 Worker、共用 checkout，並要求使用原生的隔離 subagent。這與「多帳號並行 + `claude -p`」的要求衝突。
- 替代方案：先修改 skill 支援 `claude -p` 與並行。這會在重建之前多出一段要做又要丟的工作。
- 影響：票面的 `Status:` 沿用 triage 用語（ready-for-agent／in_progress／completed／blocked），與舊 skill 的 pending 用語不同。
- 審閱：認同 2026-10-22

### AI-12：驗證交給獨立的 run
- 重要度：中
- 時間／階段：2026-10-22，拆票
- 決策：每張票實作後，另外派一個新的 run 做唯讀驗證；可以用同一個帳號。
- 理由：對應 spec 的「作者摘要不是驗收證據」，也延續舊 skill 讓 worker 與 verifier 分開的做法。
- 替代方案：主控自己驗證（會用到主帳號額度），或要求不同帳號驗證（排程更複雜）。
- 影響：每張票至少消耗 2 個 run。
- 審閱：未審

### AI-13：每張票最多修正 3 次
- 重要度：低
- 時間／階段：2026-10-22，拆票
- 決策：驗證不通過時，最多再派 3 次修正 run；超過就把票標成 blocked，跳到下一張。
- 理由：和 spec 的 D-16 一致，避免無限循環。
- 替代方案：不設上限。
- 影響：被 blocked 的票，會連帶擋住依賴它的票。
- 審閱：未審

### AI-14：watchdog 等待逾時後的處理
- 重要度：中
- 時間／階段：2026-10-22，拆票
- 決策：run 因為等待超過 5 小時而失敗時，不計入修正次數，換一個帳號、以新的 run 從該票分支目前的 commit 接手。
- 理由：額度中斷不應消耗修正預算，這是 spec 的核心原則之一。
- 替代方案：一律算成失敗。
- 影響：新 run 要從分支狀態重新理解進度，可能重做部分工作。
- 審閱：未審

### AI-15：run 記帳的位置
- 重要度：低
- 時間／階段：2026-10-22，拆票
- 決策：記在 `.scratch/platform-rebuild-v1/.orchestration/`，包含 run 指示檔、run 紀錄檔與 `runs.md` 總表。
- 理由：和 feature 放在一起，主控重啟後可以從檔案還原狀態，不依賴對話記憶。
- 替代方案：沿用上一個專案的 `accounts.json` 格式。
- 影響：無。
- 審閱：未審

### AI-16：跳過拆票提問，直接發布
- 重要度：中
- 時間／階段：2026-10-22，拆票
- 決策：to-tickets 流程原本要先請使用者確認拆分的粗細與相依關係；因為使用者要求「遇到問題先自己決定」，所以直接發布 52 張票。
- 理由：使用者明確指示。
- 替代方案：先提問，等使用者回覆再發布。
- 影響：粗細或相依關係如果不合適，要在實作開始前或過程中由使用者修正。
- 審閱：未審

### AI-17：票的粒度與排序
- 重要度：中
- 時間／階段：2026-10-22，拆票
- 決策：依 spec 的四個階段，切成 50 張 AFK 票加 2 張 ready-for-human 票（51 平台驗收、52 改名收尾），每張票都是穿過資料、Runtime、API、UI 與測試的垂直切片。第 01 張是骨架加 Runtime harness，除了 02（GLOSSARY 與 ADR）之外全部直接或間接依賴它。第 51 張依賴所有 AFK 葉節點票。
- 理由：每張票都要能放進一個新的 context window；主要測試 seam（D-04）必須最先存在。
- 替代方案：較粗的約 20 張（每張容易超出一個 context），或依層切分（違反垂直切片原則）。
- 影響：第 01 張是單點瓶頸；它完成之前，只有 02 能同時進行。
- 審閱：未審

### AI-18：票面的相依格式
- 重要度：低
- 時間／階段：2026-10-22，拆票
- 決策：票面頂端使用 `Status:` 與 `Blocked by: NN, NN` 兩行，格式依 docs/agents/issue-tracker.md。
- 理由：主控可以用機械方式計算 frontier。
- 替代方案：舊 skill 使用的 `Depends on:` 檔名格式。
- 影響：無。
- 審閱：未審

### AI-19：在 AGENTS.md 加上執行方式的指引
- 重要度：低
- 時間／階段：2026-10-22，拆票
- 決策：在 AGENTS.md 加一小節，指向本 feature 的 execution.md 與 ai-decisions.md。
- 理由：讓未來任何執行這個 feature 的 session 都會讀到「不可使用 Work-0」等限制。
- 替代方案：只寫在 `.scratch` 裡，靠使用者每次提醒。
- 影響：AGENTS.md 多出一段 feature 專屬內容；feature 完成後應移除。
- 審閱：未審

### AI-20：額度查詢改用 usage.py
- 重要度：高
- 時間／階段：2026-10-22，實作開始前
- 決策：派出前檢查一律執行 `python -I .scratch/platform-rebuild-v1/.orchestration/usage.py`，不再使用 `claude -p "/usage"`。本條取代 AI-06 的查詢方式；AI-05 的「不可用」判斷改為看腳本輸出的 `err` 或缺少百分比。
- 理由：Claude Code 2.1.307 在 `-p` 模式下的 `/usage` 只回傳花費摘要（`Total cost: …`），沒有 5h／7d 百分比，所有帳號都會被判為不可用。使用者提供的腳本讀各帳號的 OAuth 憑證查 usage 端點，可取得兩個百分比與重置時間。
- 替代方案：互動模式手動查詢；或從 run 的 stream-json 限流事件推斷。
- 影響：腳本依賴非公開的 `/api/oauth/usage` 端點與 `.credentials.json` 格式；改版時會全部回報 `err`，主控必須回報並暫停。
- 審閱：認同 2026-10-22（使用者指定）

### AI-21：CLAUDE_CONFIG_DIR 要傳 Windows 路徑
- 重要度：高
- 時間／階段：2026-10-22，實作開始
- 決策：`dispatch.sh` 以 `cygpath -w` 把 `CLAUDE_CONFIG_DIR` 轉成 `C:\Users\...` 形式再傳給 claude。
- 理由：`MSYS_NO_PATHCONV=1` 會一併停止環境變數的路徑轉換，Windows 版 claude 把 `/c/Users/...` 解讀成 `C:\c\Users\...`，讀到全新的空設定，R001、R002 因此以「Not logged in」失敗。先前用 `/usage` 驗證帳號時也受影響（它是本機指令，不需登入，所以沒有發現）。
- 替代方案：拿掉 `MSYS_NO_PATHCONV`；但 prompt 中以 `/` 開頭的字串可能被轉換。
- 影響：`C:\c\Users\User\.claude-Work-*` 留有誤建的設定目錄（約 288 KB），可以刪除。
- 審閱：認同 2026-10-22

### AI-22：D-04、D-05 的 ADR 仍標示「AI 暫定，待使用者確認」
- 重要度：中
- 時間／階段：2026-10-22，票 02 實作
- 決策：`docs/adr/0012`（D-04）與 `0013`（D-05）的狀態寫成「AI 暫定，待使用者確認」，並在後果中註明本檔「Spec 決策審閱紀錄」已有使用者 2026-10-22 的認同。DR-20261021-01～08 與 DR-20261022-01～03 的 ADR 標示為「已確認（使用者核准）」，其中 spec 為這些 DR 補充的 Claude 決策（D-06、D-08～D-11、D-16、D-22～D-25、D-31）另以「Spec 補充（AI 暫定，待使用者確認）」逐段標明。
- 理由：票 02 驗收條件明定 spec 標示的 Claude 決策若寫成 ADR，須標示為「AI 暫定，待使用者確認」；而審閱紀錄的認同是對 spec 條目的，不是對 ADR 文件的。兩者以註記並存，不擅自改標為已確認。
- 替代方案：依審閱紀錄把 D-04、D-05 標為已確認。這會違反票面驗收條件，且 ADR 的措辭使用者尚未看過。
- 影響：使用者在 ADR 層級確認後，把兩份 ADR 的狀態改為已確認並移除後果中的註記即可。
- 審閱：未審

### AI-23：ADR 編號、檔名與 DR 內容來源
- 重要度：低
- 時間／階段：2026-10-22，票 02 實作
- 決策：ADR 檔名採 `docs/adr/NNNN-<英文 slug>.md`（符合 `docs/agents/domain.md` 範例），依序為 0001～0008 對應 DR-20261021-01～08、0009～0011 對應 DR-20261022-01～03、0012 對應 D-04、0013 對應 D-05；檔頭以「來源識別」欄位標明對應的 DR 或 D 編號。DR 的背景、決策、替代方案與限制，取自 `gh issue view 3 --repo Anuise/issue-orchestrator` 所得的 issue #3 全文（§17 與各章節），不是只靠 spec 的摘要。連結以 issue #3 網址加章節編號表示，不使用標題錨點。
- 理由：spec 只引用 DR 編號而不重述內容，要寫出有依據的背景與替代方案必須讀 issue 原文；GitHub 標題錨點對中文標題不穩定，章節編號較不易失效。
- 替代方案：把 issue 全文複製進 repo（內容過多且會與 issue 分歧）；以 DR 編號作為檔名（不符合 ADR 慣例的流水號）。
- 影響：GLOSSARY.md 也依 issue #3 的詞彙與定義撰寫，並額外收錄 Account Profile、Guardian、Stop Intent、Admission 等容易混淆的詞；Glossary 刻意不放技術選型，技術決策只在 ADR。
- 審閱：未審

### AI-24：在含 C++ 工具元件的 VS 環境中執行 cargo（scripts/with-msvc.ps1）
- 重要度：中
- 時間／階段：2026-10-22，票 01 實作
- 決策：新增 `scripts/with-msvc.ps1`：以 `vswhere -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64` 找到有 C++ x64 工具元件的 Visual Studio，載入 `vcvars64.bat` 的環境後再執行指令；`pnpm build`、`pnpm test`（`scripts/lib.mjs`）一律經由它執行 cargo 與 `tauri build`。找不到時只警告、直接執行指令。
- 理由：這台機器上 rustc 會挑到 Visual Studio Community 2026（`VC\Tools\MSVC\14.51`），但它沒有安裝 C++ 桌面工作負載，連結任何 crate 都失敗（`LNK1104: msvcrt.lib`）；只有 VS Build Tools 2019 有完整函式庫。不修改系統或使用者層級的 cargo 設定（只能改 worktree 內檔案）。
- 替代方案：在 repo 內放 `.cargo/config.toml` 指定 linker 路徑（綁死本機路徑，且仍缺 `LIB`）；要求使用者安裝 VS 工作負載（無法在這個 run 內完成）。
- 影響：之後的 Worker／驗證者若直接執行裸 `cargo`，在這台機器會連結失敗，請改用 `pnpm test`、`pnpm build` 或 `powershell -NoProfile -File scripts/with-msvc.ps1 cargo …`。已寫入 `docs/development.md`。
- 審閱：未審

### AI-25：Runtime 單實例與預設埠、資料目錄的決定方式
- 重要度：中
- 時間／階段：2026-10-22，票 01 實作
- 決策：Runtime 以具名 mutex 取得單實例鎖，名稱為 `Local\Baton.Runtime.<資料目錄正規化後的 FNV-1a 雜湊>`，也就是「每個資料目錄一個 Runtime」。預設資料目錄 `%LOCALAPPDATA%\Baton`、預設埠固定 `47821`（spec D-23 的「既定埠」），可由 `BATON_DATA_DIR`、`BATON_PORT` 覆寫。本票只綁 IPv4 loopback（`127.0.0.1`）；IPv6 雙棧、Host／Origin 檢查與憑證留給票 04。
- 理由：spec 只說「單實例，具名 mutex」，沒有規定命名。以資料目錄為單位同時符合「同一個 SQLite 只有一個 writer」的原意，也讓測試與冒煙測試能用不同暫存資料目錄並存，不必動到使用者的真實 Runtime。
- 替代方案：固定全域名稱 `Local\Baton.Runtime`（測試無法並行、冒煙測試會碰到使用者真實的 Runtime）；以埠作為單實例鍵（兩個資料目錄搶同一個埠時行為不直觀）。
- 影響：票 04、25、31～33 若要換埠、換探索機制（例如生命週期檔記錄實際埠），需同步調整 `RuntimeConfig` 與 `baton-client`。FNV-1a 是自己實作的穩定雜湊，之後改名資料目錄名稱會連帶改變 mutex 名稱（需要的話，產品改名時一併處理）。
- 審閱：未審

### AI-26：Rust workspace 的 crate 切分
- 重要度：中
- 時間／階段：2026-10-22，票 01 實作
- 決策：切成 `baton-domain`（純邏輯：產品常數、時間戳、Runtime 設定解析）、`baton-ports`（六個 Port trait 與值型別）、`baton-store`、`baton-api`、`baton-runtime`（組裝與正式執行檔）、`baton-harness`（測試 harness 與 fake）、`baton-client`（桌面殼用的連線／啟動邏輯），再加上 `src-tauri` 的 `baton-desktop`。Port trait 獨立成 crate，讓 API 與 Runtime 共用而不產生循環相依；Port 使用 `async-trait` 與 `futures::BoxStream`，以便存成 `Arc<dyn …>`。Runtime 設定解析放在 domain 並只提供 `from_lookup`（環境變數由呼叫端注入），避免 `baton-client` 與 `baton-runtime` 循環相依。
- 理由：spec 列了 Domain、Store、Runtime、API 四個深模組；Ports 與 harness 是 D-04 要求的 seam，需要被多個 crate 共用。
- 替代方案：把 Port 放進 `baton-domain`（讓純邏輯 crate 帶 async 與 futures 相依）；把 harness 做成 `baton-runtime` 的 `cfg(test)` 模組（其他 crate 無法重用）。
- 影響：後續票新增模組時，Domain 狀態機放 `baton-domain`，Port 的新方法放 `baton-ports`，情境測試放 `baton-harness/tests/`。
- 審閱：未審

### AI-27：Web UI 經由 Tauri 指令查詢 Runtime，不從 WebView 直接打 HTTP
- 重要度：中
- 時間／階段：2026-10-22，票 01 實作
- 決策：桌面殼提供 `runtime_health` Tauri 指令（在殼內用 `baton-client` 查詢 Runtime），Web UI 在 Tauri 內呼叫它；單純用瀏覽器開發時（沒有 `__TAURI_INTERNALS__`）才改用 `fetch('/api/v1/health')`，由 Vite 開發伺服器代理到 `127.0.0.1:47821`。
- 理由：spec D-23 要求不回應任何 CORS 預檢，而 Tauri 的 WebView origin 與 Runtime 不同源，直接 `fetch` 會被擋；且 D-22 規定管理憑證由 App 讀取、使用者不碰密鑰，憑證只該在殼內處理。
- 替代方案：在 Runtime 對 Tauri origin 開放 CORS（與 D-23 衝突）；把 token 傳給 WebView（擴大憑證暴露面）。
- 影響：票 04 加上憑證後，只需在殼內的指令加上 Authorization 標頭，Web UI 不用改。之後的 API 呼叫也都應走同一條路。SSE 串流（D-21）需要另外設計（可能由殼轉送事件）。
- 審閱：未審

### AI-28：App 與 Runtime 的生命週期關係（本票範圍）
- 重要度：中
- 時間／階段：2026-10-22，票 01 實作
- 決策：App 啟動時（`setup` 與第一次 `runtime_health`，以 `OnceCell` 保證只做一次）先查健康端點；連得上就連回，連不上才以脫離程序的方式（`CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`，stdio 全部丟棄）啟動與 App 同資料夾的 `baton-runtime.exe` 並等最多 20 秒。關閉 App 視窗不會停止 Runtime。Runtime 之後若意外結束，App 目前只會顯示「Runtime 未連線」與原因，不會自動重啟。
- 理由：符合 DR-01「關窗工作繼續」與「單實例」；自動重啟與完全退出語意分別屬於票 25（完全退出、停止意圖）與票 31～33（崩潰恢復、監護），現在做會與停止意圖衝突。
- 替代方案：每次輪詢失敗都嘗試重啟 Runtime（會在票 25 的『完全退出』之後違反使用者的停止意圖）。
- 影響：票 25、31～33 需要改寫 `RuntimeLink::ensure_started` 的語意。`scripts/smoke-desktop.mjs` 以真實 App 驗證了「啟動、關窗後仍在、再開連回同一個」。
- 審閱：未審

### AI-29：健康端點內容與 Store 的 runtime_boot 記錄
- 重要度：低
- 時間／階段：2026-10-22，票 01 實作
- 決策：`GET /api/v1/health` 回傳 `status`、`product`、`version`、`boot_count`、`started_at`（取自注入的 Clock）、`pid`。Store 以 `runtime_boot` 表記錄每次 Runtime 啟動（`boot_count` = 資料目錄累計啟動次數）。
- 理由：票只要求『健康端點回應、重建後仍回應』；`boot_count` 讓『重建後資料目錄確實被保留』可以被外部行為觀察到，`started_at` 證明 fake Clock 真的被注入，`pid` 讓冒煙測試能確認連回的是同一個程序。
- 替代方案：只回傳 `status: ok`（無法證明資料目錄與 Clock 注入）。
- 影響：`runtime_boot` 是第一個 migration（`PRAGMA user_version = 1`）；票 05 的完整資料表可在其後追加或重整。
- 審閱：未審

### AI-30：SQLite 初始 PRAGMA 值
- 重要度：低
- 時間／階段：2026-10-22，票 01 實作
- 決策：開啟資料庫時設定 `journal_mode=WAL`（並讀回驗證，不符則啟動失敗）、`synchronous=FULL`、`foreign_keys=ON`、busy timeout 5 秒；使用 `rusqlite` 的 `bundled` SQLite。
- 理由：spec 引用 issue #3 §11.1 的 PRAGMA 初始值，但本 run 讀不到該 issue；先採保守且常見的設定（FULL 以耐久性優先）。
- 替代方案：`synchronous=NORMAL`（WAL 下較快，但斷電時可能遺失最近的提交）。
- 影響：票 05 接手 Store 時請對照 issue #3 §11.1 修正；單一 writer 佇列也尚未實作（目前是 `Mutex<Connection>` + `spawn_blocking`）。
- 審閱：未審

### AI-31：桌面殼不納入預設的 cargo 成員，冒煙測試不納入 pnpm test
- 重要度：低
- 時間／階段：2026-10-22，票 01 實作
- 決策：`default-members` 排除 `src-tauri`（`baton-desktop` 編譯時需要已建置的 `web/dist`），`pnpm test` 只跑其餘 crate 與前端測試；桌面 App 的端到端驗證放在 `scripts/smoke-desktop.mjs`（會開視窗，經由 WebView2 遠端除錯埠讀取畫面），需先 `pnpm build` 再手動執行。CI 執行 `pnpm test` 與 `pnpm build`。
- 理由：讓 `cargo test` 在沒有前端建置產物時也能直接跑；視窗型測試不適合在一般測試指令內執行。
- 替代方案：把冒煙測試接進 `pnpm test`（需要桌面工作階段，在無頭 CI 或遠端工作階段會失敗）。
- 影響：票 34（安裝程式）若要自動驗證安裝流程，需另外設計；UI E2E（Playwright）是後續票的範圍。
- 審閱：未審

### AI-32：舊 skill 封存 tag 與 NSIS 安裝設定的細節
- 重要度：低
- 時間／階段：2026-10-22，票 01 實作
- 決策：以註解 tag `archive/legacy-skill` 指向 `main` 目前的 commit（`2317092`），`skills/` 與 README 在本分支上與該 tag 完全相同。安裝設定：`tauri.conf.json` 的 `bundle.targets = ["nsis"]`、`installMode = "currentUser"`、語言 TradChinese／English；Runtime 以 sidecar（`tauri.bundle.conf.json` 的 `externalBin`，只在 `pnpm build --bundle` 時合併）一起打包，安裝後與主程式同資料夾。圖示是程式產生的占位圖。
- 理由：Git tag 屬於整個 repository（所有 worktree 共用），建立 tag 不會動到 `main` 或 `rebuild/v1`。`pnpm build --bundle` 已實際產出 `Baton_0.1.0_x64-setup.exe`，且安裝腳本為 `RequestExecutionLevel user`，但沒有實際執行安裝與解除安裝。
- 替代方案：把 `externalBin` 放進主設定（沒有先建好 sidecar 時，一般 `cargo build` 會失敗）。
- 影響：安裝與解除安裝的實測、HKCU Run 機碼與停止流程屬於票 33、34。換正式圖示時替換 `src-tauri/icons/source.png` 後執行 `pnpm tauri icon src-tauri/icons/source.png`。
- 審閱：未審

### AI-33：PlatformInfo Port 只回報事實，支援狀態由 Domain 判定
- 重要度：中
- 時間／階段：2026-10-22，票 03 實作
- 決策：`PlatformSnapshot` 改為純事實（`os_family`、`os_build`、`feature_version`、`edition`、`native_arch`、`process_arch`），移除原本的 `support_status` 欄位；支援狀態（`Supported`／`Unsupported`／`Unknown`）、警告與支援結束日由 `baton-domain::platform::assess(snapshot, now)` 判定，結果透過 `GET /api/v1/platform` 提供。`PlatformSnapshot`、`SupportStatus` 等型別搬到 `baton-domain`，`baton-ports` 轉出。
- 理由：spec 的 Port 草圖把 `support_status` 放在 Port 內，但票面要求用 harness 驗證「四種組合、Windows 10、Server、ARM、已到期」的判定結果；若判定在 Port 內，fake 只是回報答案，測的不是判定邏輯。判定依賴 Clock（到期），放在 Domain 才能用 fake Clock 推進時間。
- 替代方案：保留草圖形狀，由真實實作自己判定（到期判斷無法用 fake Clock 測試）。
- 影響：後續票需要平台狀態時呼叫 `assess` 或讀 `/api/v1/platform`，不要自行比對版本字串。`health_demo.rs` 與 `FakePlatformInfo` 的欄位隨之更新。
- 審閱：未審

### AI-34：支援期限表的內容、來源與推估日期
- 重要度：中
- 時間／階段：2026-10-22，票 03 實作
- 決策：內建表只有兩列：25H2（build 26200）Home／Pro 服務結束日 2027-10-12；26H2（build 26300）Home／Pro 服務結束日 2028-10-10。來源標示為 Microsoft Windows 11 release information 頁，查閱日期 2026-10-22。26H2 的日期在 Microsoft 官方頁面上尚未確認，是依「發行日起 24 個月」的慣例推估，表中以 `estimated: true` 標記。其他 Windows 11 功能版本（例如 24H2）判為非正式支援（`unverified_version`），不列入表。服務結束日（UTC）當天起即視為到期。
- 理由：本 run 只能用網路搜尋取得次級來源（25H2 的 2027-10-12 各來源一致；26H2 的 2028-10-10 各來源說法為推估、且不一致於發行日），無法確認官方頁面。DR-08 規定只有驗證過的 25H2、26H2 為正式支援。
- 替代方案：26H2 不填日期（失去到期警告）；把 24H2 也放進表（它不是驗證目標，會與「僅 25H2、26H2 正式支援」衝突）。
- 影響：發布前須對照官方頁面確認 26H2 日期並更新 `SUPPORT_TABLE_CHECKED_ON`（票 51 的平台驗收可一併處理）。時區上以 UTC 日期比較，與 Microsoft 的太平洋時間最多差一天。
- 審閱：未審

### AI-35：「不阻擋派工」以 `blocks_dispatch: false` 表示，到期版本的狀態為非正式支援
- 重要度：中
- 時間／階段：2026-10-22，票 03 實作
- 決策：已到期的版本回報 `status = unsupported`、警告代碼 `support_ended`（訊息註明 L1、不阻擋派工）；所有判定結果的 `blocks_dispatch` 恆為 `false`，由 harness 測試驗證。派工、交接與恢復功能還不存在，所以本票無法測「實際派工不被擋」。
- 理由：ADR-0008 的 L1 是「顯示警告，但不單憑日期停止 Run，也不禁止新派工、交接或恢復」，並描述已到期版本為「退出正式支援」。目前能被外部觀察的只有 API 回應。
- 替代方案：新增 `expired` 狀態（讓狀態列多一種顯示，但 spec 只有「正式／非正式」兩類）。
- 影響：Scheduler、交接與恢復相關的票不得以平台狀態作為准入條件；要在對應票補上「到期平台仍可派工」的情境測試。
- 審閱：未審

### AI-36：Windows 11 與 ARM 模擬的辨識方式
- 重要度：中
- 時間／階段：2026-10-22，票 03 實作
- 決策：以 build 編號 ≥ 22000 判定 Windows 11（登錄 `InstallationType` 以 `Server` 開頭則為 Server；其餘 build < 22000 且主版本 10 為 Windows 10）。原生架構取自 `IsWow64Process2` 的 nativeMachine，程序架構取編譯目標架構，兩者不同代表模擬；原生架構不是 x64 一律非正式支援。版本別由 `EditionID` 正規化（`Core*`→Home、`Professional`／`ProfessionalN`→Pro，其餘保留原值）。讀不到 build 或原生架構時回報 `unknown`，狀態為未知，不當作支援。
- 理由：實測本機登錄 `ProductName` 是「Windows 10 Pro」，但 build 26300、`DisplayVersion` 26H2，確為 Windows 11；用名稱判定會誤判。x64 程序在 ARM 機器上看到的環境變數與 `GetNativeSystemInfo` 可能是模擬後的值，`IsWow64Process2` 較可靠。
- 替代方案：用 `RtlGetVersion` 或 WMI（多一層相依、同樣要靠 build 編號）。
- 影響：本 run 沒有 ARM 機器，ARM 上 x64 模擬的實機行為只由純函式的映射測試與 harness 情境覆蓋；票 51 平台驗收的清單應包含實機確認。
- 審閱：未審

### AI-37：平台狀態在 UI 的呈現位置
- 重要度：低
- 時間／階段：2026-10-22，票 03 實作
- 決策：狀態列在 Runtime 運作中時多顯示「平台：正式支援／非正式支援／未知」；平台狀態頁（`PlatformPage`）先放在 App 主區，列出診斷事實、警告（`role="alert"`，並說明不阻擋使用）與支援期限表的來源與查閱日期。讀取平台失敗時顯示「未知」，不影響 Runtime 狀態。Tauri 殼新增 `runtime_platform` 指令，與 `runtime_health` 同樣經由殼查詢（AI-27）。`scripts/smoke-desktop.mjs` 加入對平台狀態的檢查，並已用真實 App 跑過。
- 理由：D-21 規定左側有「平台狀態」入口，但導覽結構尚未存在；先確保內容可見，之後的票把它掛到導覽即可。
- 替代方案：先建立左側導覽（超出本票範圍）。
- 影響：加入導覽的票只需移動 `PlatformPage` 的掛載位置。
- 審閱：未審

### AI-38：桌面憑證的交付檔、輪替與有效期
- 重要度：高
- 時間／階段：2026-10-22，票 04 實作
- 決策：Runtime 每次啟動都產生新的 256 位元桌面 token（64 個十六進位字元），資料庫 `credential` 表只存 SHA-256 雜湊（不加鹽，因 token 本身是高熵隨機值），並撤銷所有舊的桌面憑證。明文以 DPAPI（目前使用者範圍，`CRYPTPROTECT_UI_FORBIDDEN`）保護後原子寫入 `<資料目錄>\auth\desktop-credential.bin`，且在開始監聽**之前**完成，所以 App 連得上時讀到的一定是目前有效的憑證。桌面憑證沒有時間上限（`expires_at = NULL`），失效完全靠「下次啟動再輪替」；`expires_at` 欄位與過期檢查已完成，供 Agent 憑證與測試使用。桌面 App（`RuntimeClient`）每次呼叫都重新讀檔，所以 Runtime 重啟後自動換用新憑證；被拒絕時回報 Runtime 的原因代碼，沒有任何不帶憑證的重試。寫檔或輪替失敗會讓 Runtime 啟動失敗，不退回免驗證。
- 理由：spec D-22 只說「輪替並寫入 DPAPI 保護檔」，沒有規定檔名、有效期與讀取時機。每次讀檔最單純，且沒有「App 持有舊憑證」的狀態要管理。桌面憑證設 TTL 會讓常駐數天的 Runtime 在中途失效，卻沒有刷新機制。
- 替代方案：桌面憑證設 24 小時 TTL 並由 App 要求刷新（需要額外端點與狀態）；App 啟動時讀一次並快取（Runtime 重啟後需要額外的重連邏輯）。
- 影響：DPAPI 實測已涵蓋：真實 DPAPI 往返與密文不含明文（`baton-credential` 單元測試）、真實 `baton-runtime.exe` 重啟後輪替且 App 以 DPAPI 檔重連（`single_instance_process.rs`）、桌面 App 冒煙測試。「DPAPI 憑證交付」這項 Validation Required 在目前使用者範圍已有實測證據；跨使用者／服務帳號情境未驗證。若之後要讓 Runtime 以別的帳號執行，需改交付方式。
- 審閱：認同 2026-10-22

### AI-39：入口防護的具體規則與錯誤碼
- 重要度：高
- 時間／階段：2026-10-22，票 04 實作
- 決策：`guard` 套在整個 Router（含 SSE 與未知路徑），檢查順序為 Host → CORS 預檢 → Origin → 固定標頭 → bearer 憑證 → 角色。(1) Host 只接受 IP 字面值 `127.0.0.1:<實際埠>` 與 `[::1]:<實際埠>`，**不接受 `localhost`**、沒有埠、其他埠或其他位址。(2) 有 `Origin` 時只接受 `http://tauri.localhost`、`https://tauri.localhost`、`tauri://localhost`（`null` 與其他一律拒絕）；沒有 `Origin`（桌面殼內的原生請求）放行。(3) 任何 `OPTIONS` 回 405，且所有回應都不帶 `Access-Control-*`。(4) 固定標頭為 `X-Baton-Client: baton-desktop`。(5) 回應為 JSON `{"error": <代碼>, "message": <固定中文說明>}`：403 `host_not_allowed`／`origin_not_allowed`／`client_header_missing`／`role_not_allowed`、405 `preflight_refused`、401 `credential_missing`／`credential_invalid`／`credential_expired`、500 `internal_error`。訊息是固定文字，不回顯請求內容。(6) 通過驗證才進路由，所以未驗證者看不到 404（無法探測路由）。
- 理由：spec D-23 只列出機制，沒有規定細節。只收 IP 字面值能避免 `localhost` 名稱解析被利用，且桌面殼本來就以 `127.0.0.1` 連線。Tauri 在 Windows 的 WebView origin 是 `http://tauri.localhost`（啟用 https 方案時為 https），其他平台為 `tauri://localhost`，三種都列入。
- 替代方案：接受 `localhost`（對 DNS rebinding 較寬鬆，且 Vite 開發代理也不會因此變得可用）；把 CORS 預檢回 204 但不帶標頭（瀏覽器仍會視為失敗，但違反「不回應預檢」）。
- 影響：用 `pnpm --filter web dev` 在一般瀏覽器開 Web UI 時，請求被拒（狀態列顯示「Runtime 回應 403」）；這是設計結果，Web UI 只能在桌面 App 內操作（`pnpm tauri dev`），已寫入 `docs/development.md`。若 Tauri 的 origin 實測與上述不同，只需改 `crates/baton-api/src/guard.rs` 的 `APP_ORIGINS`。冒煙測試已確認真實 WebView2 內的 App 能正常連線。
- 審閱：認同 2026-10-22

### AI-40：為了驗證「串流也要驗證」，加入最小的 SSE 端點 `/api/v1/events`
- 重要度：中
- 時間／階段：2026-10-22，票 04 實作
- 決策：新增 `GET /api/v1/events`：通過驗證後先送出一個 `ready` 事件，之後只有保活訊息；Runtime 開始停止時串流主動結束（避免優雅停止被長連線卡住）。沒有任何業務事件。
- 理由：票要求「所有端點（含 SSE 串流）」與「串流也被拒絕」，但目前 repo 沒有任何串流端點，沒有東西可以測。這是能驗證該條件的最小實作。
- 替代方案：不加端點、只對一般端點驗證（驗收條件無法被證明）；等有真實事件的票再補（那時才發現串流沒套防護的風險較高）。
- 影響：後續真正的事件串流票直接沿用這個路徑與防護，只需改 handler 的內容。連線建立後若憑證被撤銷，已開著的串流不會被中途切斷（Runtime 重啟會結束全部串流）；Agent 憑證在 Run 結束時要切斷串流屬後續票。
- 審閱：未審

### AI-41：角色欄位的範圍與憑證簽發入口
- 重要度：中
- 時間／階段：2026-10-22，票 04 實作
- 決策：`credential` 表有 `token_hash`、`role`（`desktop`／`agent`）、`issued_at`、`expires_at` 四欄。目前所有端點都是管理 API，只有桌面角色可用，Agent 角色憑證得到 403 `role_not_allowed`。`RunningRuntime::credentials().issue(role, ttl)` 是簽發入口（不影響既有憑證），Run 准入由後續的票呼叫；spec D-22 提到的 Workspace、Run、世代欄位**沒有**預先建立，由後續票以新的 migration 加入。
- 理由：票只要求「預留角色欄位」；其餘欄位的語意取決於 Run 狀態機（票 05 之後），現在猜會做錯。以 403 而非放行是安全的預設：沒有 Agent API 以前，Agent 憑證不該能碰任何東西。
- 替代方案：現在就建完整的欄位（可能與之後的資料模型衝突）；讓 Agent 憑證可存取唯讀端點（沒有需求）。
- 影響：Agent API 的票需要把路由分成管理與 Agent 兩組，並依路由指定所需角色（目前 `guard` 內是寫死的桌面角色檢查）。
- 審閱：未審

### AI-42：雙棧 loopback 綁定的邊界情況
- 重要度：中
- 時間／階段：2026-10-22，票 04 實作
- 決策：Runtime 同時綁 `127.0.0.1:<port>` 與 `[::1]:<port>`（同一個埠）。`RuntimeConfig.bind` 若不是 loopback（如 `0.0.0.0`、`[::]`、區網位址）直接啟動失敗（`NotLoopback`）。IPv6 綁定因「本機沒有 IPv6」以外的原因失敗時只服務 IPv4；但 `[::1]` 的同一個埠已被別的程序占用（`AddrInUse`）視為啟動失敗，不悄悄讓出那個位址。埠 0（測試用）時若 IPv6 同埠被占用會重選埠，最多 16 次。
- 理由：spec 要求同時綁定兩者且不監聽其他位址，但沒說 IPv6 被停用的機器要怎麼辦；不能讓這種機器完全無法使用，但被占用則可能是惡意程序搶位址。
- 替代方案：IPv6 不可用就整個啟動失敗（部分企業機器停用 IPv6 時無法使用）；IPv6 被占用也只退回 IPv4（讓他人程序可在 `[::1]` 冒充 Runtime）。
- 影響：App 一律用 `127.0.0.1` 連線，所以只服務 IPv4 時 App 不受影響。
- 審閱：未審

### AI-43：Runtime 日誌檔與 SecretProtector Port、`baton-credential` crate
- 重要度：中
- 時間／階段：2026-10-22，票 04 實作
- 決策：(1) 新增 `<資料目錄>\logs\runtime.log`（附加寫入，每行以 Clock Port 的毫秒時間戳開頭），目前記錄 Runtime 啟動與每個請求的「方法、路由樣板、狀態碼、結果或拒絕代碼」；路徑記的是路由樣板（未匹配為 `(unmatched)`），不含實際 URL、查詢字串或標頭，所以即使有人把 token 放進 URL 也不會進日誌。(2) 新增 `SecretProtector` Port（`protect`／`unprotect`），正式實作為 DPAPI，harness 以可逆的 fake 取代，測試可重現「讀取保護檔」的流程。(3) 檔案讀寫與 DPAPI 放在新 crate `baton-credential`，因為 Runtime（寫入）與桌面殼／client（讀取）都需要它，放在任一邊都會造成不自然的相依。(4) 憑證不外洩由 `api_auth.rs` 的日誌掃描測試驗證：對各種拒絕與成功路徑（含把 token 放進 URL、Origin、Host）發出請求後，掃描日誌、回應標頭與回應本文，以及資料庫與 WAL 檔，都不得出現任何明文 token。
- 理由：票要求「測試以日誌掃描驗證」，但先前沒有日誌；D-17 規定日誌放在資料目錄的子目錄。DPAPI 是外部副作用，依 D-04 應是 Port。
- 替代方案：用 `tracing` 加全域 subscriber（測試之間難以隔離）；只記標準輸出（無法掃描、關閉主控台後就消失）。
- 影響：日誌輪替、保留期限與可見度屬後續的票；目前檔案會一直增長。後續票新增日誌行時務必不要寫入標頭或 URL。
- 審閱：未審

### AI-44：「資料位於網路共享或雲端同步資料夾時拒絕」的對象是 Baton 資料目錄
- 重要度：中
- 時間／階段：2026-10-22，票 05 實作
- 決策：票上這句話解讀為 spec D-17 的「Baton 本機資料（控制資料庫所在的資料目錄）不可放在網路共享或雲端同步資料夾」，檢查發生在 `Store::open`（Runtime 啟動時），桌面殼在啟動 Runtime 之前也呼叫同一個 `baton_store::check_data_dir`，讓原因直接顯示在狀態列。使用者替 Workspace 選的目錄（可能是同步中的專案資料夾）不受這個限制。
- 理由：issue #3 §11.1 的限制理由是 SQLite 的檔案鎖與 WAL 共享記憶體，只對資料庫檔案成立；Workspace 目錄裡的檔案是被 Agent 讀寫的內容，使用者的專案放在 OneDrive 下很常見，一律拒絕會讓產品不可用。
- 替代方案：同時拒絕 Workspace 目錄位於網路共享或雲端同步資料夾。若使用者的意思是這個，只需要在 `workspaces::create` 呼叫 `check_data_location`，成本低。
- 影響：偵測方式見 AI-49。拒絕訊息說明原因並指示改用本機磁碟（預設 `%LOCALAPPDATA%\Baton` 或 `BATON_DATA_DIR`）。
- 審閱：未審

### AI-45：Workspace 的「選擇目錄」先用文字輸入，不做原生目錄選擇對話框
- 重要度：中
- 時間／階段：2026-10-22，票 05 實作
- 決策：左側清單下方的表單以文字欄位輸入目錄路徑（附「名稱（選填）」）；Runtime 檢查路徑是否為存在的絕對目錄並回報原因。不加入 `tauri-plugin-dialog`。
- 理由：原生對話框需要新增 Tauri 外掛、前端套件與 capability 權限，也無法在 harness 與 vitest 中測試；票的核心是資料、事件與持久化。路徑驗證在 Runtime 做，所以之後加對話框只是前端多一個按鈕，不會改 API。
- 替代方案：加入 `tauri-plugin-dialog` 的 `open({ directory: true })`。
- 影響：使用者目前必須自己貼上路徑。建議後續 UI 票補上「瀏覽…」按鈕，並在驗證清單加入手動操作確認。
- 審閱：未審

### AI-46：schema 版本改以 `schema_migrations` 記錄，較新的資料庫「可開啟、拒絕寫入」
- 重要度：中
- 時間／階段：2026-10-22，票 05 實作
- 決策：(1) 版本記錄從 `PRAGMA user_version`（票 01～04）改為 `schema_migrations(version, name, applied_at)`（issue #3 §11.2），`applied_at` 是 UTC 毫秒，所以 `Store::open` 新增 `now` 參數。票 01～04 留下的資料庫（只有 `user_version`）在第一次開啟時自動補記，不遺失資料。(2) 資料庫中最大的版本大於程式支援的版本時，`Store::open` 仍成功（不改動資料庫），但每一次寫入都在交易內重新檢查並以 `IncompatibleSchema` 拒絕，包含「開啟之後才被另一個較新程式升級」的情況；Runtime 啟動時第一個動作就是寫入（記錄啟動），所以 Runtime 會以「拒絕寫入」的說明啟動失敗。
- 理由：issue 的資料表清單明列 `schema_migrations` 含套用時間與「不相容版本拒絕寫入」；開啟時不改動、寫入時檢查，讓將來可以做「唯讀診斷／備份」而不必改 Store。
- 替代方案：開啟時直接失敗（較簡單，但無法做唯讀診斷，也擋不住開啟後才被升級的情況）；維持 `user_version`（沒有套用時間與名稱）。
- 影響：舊版程式不會悄悄用舊的資料表結構寫入新版資料。已發布的 migration 不可修改，只能往後追加。
- 審閱：未審

### AI-47：單一寫入佇列的實作與 PRAGMA 值（對照 issue #3 §11.1）
- 重要度：低
- 時間／階段：2026-10-22，票 05 實作
- 決策：Store 以一條專用的 `std::thread` 擁有唯一的寫入連線，工作經 `mpsc` 排隊，每個工作在 `BEGIN IMMEDIATE` 交易中執行，失敗或 panic 都回滾且佇列繼續運作；唯讀查詢用另一條 `query_only=ON` 的連線。所有 Store 的公開寫入方法（含票 04 的憑證與啟動記錄）都改走佇列。最後一個 `Store` 複本被丟棄時關閉佇列並等待執行緒結束，所以資料庫檔案會確實釋放（重建 Runtime 與清理暫存目錄需要）。PRAGMA 已對照 issue §11.1：`journal_mode=WAL`、`synchronous=FULL`、`foreign_keys=ON`、`busy_timeout=5000`，與 AI-30 相同，無需修正；兩條連線開啟後都讀回驗證，`Store::pragma_settings` 可查詢實際生效值。
- 理由：spec 要求「單一 writer task、所有寫入序列化為短交易」。專用執行緒讓同步的 rusqlite 不佔用 Tokio 的 blocking 池，也不需要連線鎖。
- 替代方案：`Mutex<Connection>` + `spawn_blocking`（票 01 的做法，序列化是靠鎖，不是佇列）；Tokio 的 writer task 搭配 `spawn_blocking`。
- 影響：寫入的吞吐量受單執行緒限制，符合 SQLite 單 writer 的事實。busy_timeout 逾時目前直接回報錯誤，issue 所說的「有限重試與退避」尚未實作，因為單一 writer 加上單實例鎖下不會發生，若出現再補。
- 審閱：未審

### AI-48：Workspace 的資料模型與 API 形狀
- 重要度：低
- 時間／階段：2026-10-22，票 05 實作
- 決策：(1) 識別為 `ws_` 加 96 位元隨機十六進位，由 API 層產生（Domain 不產生亂數）。(2) `workspace` 表含 `version`（每次狀態變更加一），事件表含 `aggregate_version` 與 `UNIQUE(aggregate_type, aggregate_id, aggregate_version)`，對應 issue 的「聚合版本」；事件表以觸發器禁止更新與刪除。(3) 路徑去掉前後空白與引號、統一為反斜線、去掉結尾分隔線；重複判定不分大小寫，只比對「使用中」的 Workspace，所以封存後可以重新登錄同一個目錄。(4) 名稱省略時取目錄名稱，上限 100 字元。(5) `GET /api/v1/workspaces` 回傳全部（含已封存，帶 `status`），UI 只顯示使用中的；清單依寫入順序（rowid）排列，因為同一毫秒建立的 Workspace 不能以時間或隨機 id 排序。(6) 錯誤格式沿用 `{ error, message }`，代碼：`workspace_path_invalid`、`workspace_path_not_found`、`workspace_path_not_directory`、`workspace_name_invalid`、`workspace_already_exists`、`workspace_not_found`、`workspace_already_archived`、`bad_request`、`store_incompatible`。
- 理由：都是票上沒有指定、但實作時必須選的細節；以上選擇偏向保守與可追溯。
- 替代方案：序號型識別（需要 DB 配號，Domain 命令變複雜）；列表預設排除封存（之後要看封存清單還得改 API）。
- 影響：Workspace 的授權範本、設定與自動續跑旗標（GLOSSARY 所列）由後續的票加入新欄位或新表。
- 審閱：未審

### AI-49：資料目錄位置的偵測方式與盲點
- 重要度：低
- 時間／階段：2026-10-22，票 05 實作
- 決策：判定規則是純字串邏輯（`baton-domain::storage_location`）：UNC 路徑（含 `\\?\UNC\`）視為網路共享；路徑任一段以已知同步資料夾名稱開頭（OneDrive、Dropbox、Google Drive／GoogleDrive、iCloud Drive／iCloudDrive、Box Sync、Nextcloud，不分大小寫）視為雲端同步；環境變數 `OneDrive`、`OneDriveConsumer`、`OneDriveCommercial` 指向的位置之下也視為雲端同步。Store 同時檢查「使用者給的路徑」與「`canonicalize` 解析後的真實路徑」，所以對應到網路的磁碟機代號（解析後是 `\\?\UNC\...`）也會被擋。
- 理由：不引入 Windows API 就能涵蓋最常見的情況，且可在 harness 中測試。
- 替代方案：用 `GetDriveTypeW`、檔案屬性（OneDrive 的 reparse point／雲端檔案屬性）偵測。
- 影響：已知盲點：名稱不在清單內的同步工具、使用者自訂的同步根目錄（非環境變數指向）偵測不到；真實的網路磁碟機與 OneDrive「檔案隨選」情境屬於 **Validation Required**，需要在 Windows 上手動驗證，不在自動化測試內。
- 審閱：未審

### AI-50：WorkspaceFs Port 新增 `inspect_directory`，正式版只有這一項是真實實作
- 重要度：低
- 時間／階段：2026-10-22，票 05 實作
- 決策：`WorkspaceFs` 新增 `inspect_directory(path) -> Directory | NotFound | NotADirectory`，供建立 Workspace 前確認目錄存在。檢查本身是共用的 `baton_ports::directory_state`（真實檔案系統）；harness 的 `FakeWorkspaceFs` 與正式的 `LocalWorkspaceFs` 都使用它，測試在暫存目錄中建立真實目錄即可。`LocalWorkspaceFs` 的其餘方法（worktree、dirty、原子寫入）仍回傳 `NotImplemented`，由後續的票實作。
- 理由：「目錄是否存在」是檔案系統的事實，fake 模擬它沒有價值；但它仍應經過 Port，後續若要模擬權限錯誤或逾時，可以在 fake 中腳本化。
- 替代方案：API 直接呼叫 `std::fs`（違反 D-04「外部副作用都透過 Port」）。
- 影響：`Unavailable` 不再負責 `WorkspaceFs`。
- 審閱：未審

### AI-51：harness 測試允許唯讀檢查控制資料庫
- 重要度：低
- 時間／階段：2026-10-22，票 05 實作
- 決策：Harness 新增 `db()`（唯讀開啟控制資料庫）、`send_json()`（回傳狀態碼與 JSON，不因失敗而中止）與 `HarnessBuilder::prepare_data_dir()`／`try_start()`（Runtime 首次啟動前整理資料目錄、並取得啟動錯誤）。開發指南原本寫「不要斷言資料表欄位」，本票的驗收條件（事件表追加、UTC 存放、WAL、不相容 schema 不被改動）只能由落地的內容證明，所以只在這幾類測試中使用 `db()`，其餘行為一律經由 API。
- 理由：驗收條件本身就是關於資料庫內容。
- 替代方案：新增只為測試存在的「列出事件」API（會把測試需求放進正式 API 面）。
- 影響：開發指南已補上這個例外與使用範圍。
- 審閱：未審

### AI-52：Run 尚未存在時，准入檢查以 Domain 函式、Store 方法與 `admission-check` 端點呈現
- 重要度：高
- 時間／階段：2026-10-22，票 06 實作
- 決策：票 06 的驗收條件要求「未確認授權的 Workspace 無法建立需要對應能力的 Run（准入檢查拒絕並顯示原因）」，但 Run 要到票 10 才存在。因此把判定做成 `baton_domain::authorization::admit(workspace, 目前生效的 revision, 需要的能力)`，Store 提供 `check_admission`，並新增桌面專用端點 `POST /api/v1/workspaces/{id}/admission-check`（`{ "required_capabilities": [...] }`）。通過回 200 與依據的 `authorization_revision`；被拒絕回 409（Workspace 不存在為 404）並帶穩定代碼：`authorization_unconfirmed`、`capability_not_granted`（訊息列出缺少的能力與目前 revision）、`workspace_archived`。「不需要任何能力」的 Run 不要求授權已確認（只要求 Workspace 存在且使用中）。Agent 憑證打這個端點得到 403。
- 理由：沒有 Run 就無法測「Run 被拒」，端點是在主要 seam 上能觀察准入判定的最小做法；判定本身是純函式，票 10 的派工交易直接呼叫它，不需要重寫規則。「需要哪些能力」由 Task 契約決定，屬於票 09、10，不是本票。
- 替代方案：等票 10 再測准入（本票驗收條件無法證明）；為了測試新增假的 Run 表（會與票 10 的資料模型衝突）。
- 影響：**票 10 的派工交易必須呼叫同一個 `admit`，並以 Task 需要的能力為輸入**，否則「未確認授權的 Workspace 無法建立 Run」只在端點成立。目前 `check_admission` 走唯讀連線；票 24（R1-a）需要把它移進寫入佇列的交易，才能與授權更新建立可查驗的持久化順序。
- 審閱：未審

### AI-53：能力詞彙、兩個範本的內容與高風險能力的處理
- 重要度：高
- 時間／階段：2026-10-22，票 06 實作
- 決策：(1) 能力共 12 個：`read_files`、`write_files`、`web_read`、`run_build_test`、`install_project_dependencies`，加上七個「不在 P3」的高風險能力 `admin_install`、`global_install`、`remote_write`、`deploy`、`external_message`、`payment`、`production_data`。授權的一項是「能力 + 目標資源（至少一項）+ 限制」。(2) 研究／文件範本：讀取檔案、編輯檔案、讀取公開網頁（唯讀、不登入、不提交表單）；不含建置、測試與安裝。可信專案開發（P3）範本：讀取檔案、編輯檔案、建置與測試（目標為 Workspace 目錄，限制為僅執行專案內建置測試指令、不使用系統管理員身分）、安裝專案依賴（目標為專案設定檔與 lockfile 宣告的套件來源，限制為僅安裝到專案目錄、不做全域安裝、不使用管理員身分）。範本的目標資源代入該 Workspace 的目錄。(3) 沒有任何範本含高風險能力；使用者只能在編輯時明確加入，加入後確認摘要逐項標示「高風險」，且確認必須承認風險。(4) 不替使用者預選範本，也不依目錄是否為 Git repo 推測；兩個範本並列，也可以不套用範本自行組成。
- 理由：票與 ADR-0003 只列出類別（編輯、指定環境建置測試、指定來源安裝依賴）與必須另外授權的清單，沒有給出可程式化的詞彙；「讀取公開網頁」是研究／文件工作的合理基本能力，是我加入的，屬於範本內容而非產品需求。「指定環境」「指定來源」以目標資源與限制文字表達，目前不由 Runtime 強制，強制要到 Agent 與 permission-prompt（票 11、22）才有可執行的位置。
- 替代方案：研究範本只含檔案讀寫（Agent 無法上網查資料）；依 `.git` 是否存在預選開發範本（違反「不被強制套用開發範本」的精神，且 WorkspaceFs 要多一個 Port 方法）。
- 影響：能力與限制目前只是「被保存、被准入檢查」的資料；實際攔截屬後續票。新增能力要同時更新 Domain 的 `Capability::ALL`、`label`、`is_high_risk`、`runs_trusted_code` 與 Web UI 型別。
- 審閱：未審

### AI-54：確認流程的細節（摘要、風險承認、內容未變、來源判定）
- 重要度：中
- 時間／階段：2026-10-22，票 06 實作
- 決策：(1) 新增預覽端點 `POST /api/v1/workspaces/{id}/authorization/summary`（只回傳摘要，不保存），摘要含逐項能力、目標、限制、高風險標記、必須揭露的風險與 `requires_risk_acknowledgement`；風險文字固定包含「專案與第三方依賴程式會以目前 Windows 使用者的身分執行」。(2) 授權內容含會執行專案或依賴程式的能力（建置測試、安裝依賴、管理員／全域安裝）或任何高風險能力時，`PUT /api/v1/workspaces/{id}/authorization` 必須帶 `acknowledged_risk: true`，否則 400 `risk_acknowledgement_required`；研究／文件範本不需要。承認旗標由 UI 的勾選產生，Runtime 無法證明使用者真的看過摘要。(3) 內容（排序與前後空白正規化後）與目前生效的 revision 相同時，不建立新 revision，回 200 與既有 revision；真正的新 revision 回 201。(4) 空授權（沒有任何能力）是合法的，等於撤回全部能力，會建立新 revision。(5) revision 的來源由 Runtime 判定，不採信用戶端：沒帶範本為 `custom`，帶範本且內容與該 Workspace 的範本完全一致為 `template`，否則 `template_edited`，並記錄 `base_revision`（前一個 revision）與 `confirmed_by: "desktop"`（只有桌面憑證能確認；Agent 憑證得到 403）。(6) 已封存的 Workspace 不能再修改授權（409 `workspace_archived`）。
- 理由：票要求確認摘要揭露風險、確認後才生效、每次修改有來源與時間；摘要預覽加上承認旗標是在 API 層讓「先看再確認」可被測試的最小做法。內容未變不建立 revision，避免重複點擊製造沒有意義的歷史，也符合「每次『修改』產生新 revision」。
- 替代方案：用摘要雜湊綁定確認請求（可證明確認的就是看過的內容，但需要保存摘要狀態，目前沒有需求）；內容未變也建立 revision（歷史充滿重複）；用戶端自報 `source`（可被偽造）。
- 影響：撤銷授權的介面、「仍有 N 個 Run 沿用舊授權」與 R1-a 順序屬票 24；本票的「修改」包含拿掉能力，但還沒有專門的撤銷流程。
- 審閱：未審

### AI-55：授權 revision 的儲存方式與事件
- 重要度：中
- 時間／階段：2026-10-22，票 06 實作
- 決策：migration 第 4 版 `authorization_revision(workspace_id, revision, grants, source, confirmed_at)`，主鍵為 `(workspace_id, revision)`，`grants` 與 `source` 存 JSON，時間為 UTC 毫秒；觸發器禁止更新與刪除（改授權只能追加新的 revision，目前生效的是最新一個）。同一個交易中追加事件 `authorization_confirmed`，聚合類型 `workspace_authorization`、聚合識別為 Workspace 識別、聚合版本等於 revision 編號，payload 含 grants 與 source。確認不更動 `workspace` 表（不增加 Workspace 的 `version`）。
- 理由：事件表本來就以（聚合類型、聚合識別、聚合版本）唯一，revision 編號剛好對應；不可變的 revision 讓之後「已准入的 Run 沿用當時的授權版本」（票 24）只需記下 revision 編號。授權不是 Workspace 狀態機的一部分，獨立聚合可避免每次改授權都改寫 Workspace 列。
- 替代方案：把授權塞進 `workspace` 表的 JSON 欄位（無歷史、無法追加式保存）；授權改動也遞增 Workspace 版本（兩個聚合版本互相牽動）。
- 影響：備份與還原（票 40）需涵蓋這張表；票 24 的准入以 `authorization_revision` 編號為依據。
- 審閱：未審

### AI-56：授權設定畫面的位置與操作方式
- 重要度：低
- 時間／階段：2026-10-22，票 06 實作
- 決策：授權設定畫面（`AuthorizationPanel`）放在主區上方；建立 Workspace 成功後自動開啟，既有 Workspace 在左側清單用「授權設定」按鈕重新開啟。編輯方式是每個能力一個核取方塊，勾選後出現「目標資源」與「限制」兩個文字區（每行一項）；套用範本只是把範本內容載入編輯區，之後仍可任意修改。必須先按「檢視確認摘要」才會出現「確認並保存授權」；編輯任何內容都會讓已看過的摘要與風險承認作廢。`scripts/smoke-desktop.mjs` 加入「在真實 App 內建立 Workspace、套用 P3、承認風險、確認保存，確認前後准入檢查結果不同」的檢查（同時驗證 Tauri 指令的參數對應）。
- 理由：D-21 的導覽與分頁結構尚未存在（AI-37），先確保功能可用，之後只需移動掛載位置；Tauri 預設把 JS 的 camelCase 參數對應到 Rust 的 snake_case，實際以冒煙測試確認。
- 替代方案：獨立的建立精靈步驟（D-20 的首次啟動精靈屬票 07）；用表格逐項編輯 targets（每行一項更簡單）。
- 影響：首次啟動精靈（票 07）可直接重用 `AuthorizationPanel` 作為「建立第一個 Workspace（授權範本確認）」步驟。
- 審閱：未審

## Spec 決策審閱紀錄

### D-04：主要測試 seam
- 審閱：認同 2026-10-22

### D-05：Agent 與 Runtime 的工具介面
- 審閱：認同 2026-10-22

### D-02：本 spec 範圍與交付順序
- 審閱：認同 2026-10-22
