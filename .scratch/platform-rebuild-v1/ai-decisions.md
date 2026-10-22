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

### AI-53：Run 尚未存在時，准入檢查以 Domain 函式、Store 方法與 `admission-check` 端點呈現
- 重要度：高
- 時間／階段：2026-10-22，票 06 實作
- 決策：票 06 的驗收條件要求「未確認授權的 Workspace 無法建立需要對應能力的 Run（准入檢查拒絕並顯示原因）」，但 Run 要到票 10 才存在。因此把判定做成 `baton_domain::authorization::admit(workspace, 目前生效的 revision, 需要的能力)`，Store 提供 `check_admission`，並新增桌面專用端點 `POST /api/v1/workspaces/{id}/admission-check`（`{ "required_capabilities": [...] }`）。通過回 200 與依據的 `authorization_revision`；被拒絕回 409（Workspace 不存在為 404）並帶穩定代碼：`authorization_unconfirmed`、`capability_not_granted`（訊息列出缺少的能力與目前 revision）、`workspace_archived`。「不需要任何能力」的 Run 不要求授權已確認（只要求 Workspace 存在且使用中）。Agent 憑證打這個端點得到 403。
- 理由：沒有 Run 就無法測「Run 被拒」，端點是在主要 seam 上能觀察准入判定的最小做法；判定本身是純函式，票 10 的派工交易直接呼叫它，不需要重寫規則。「需要哪些能力」由 Task 契約決定，屬於票 09、10，不是本票。
- 替代方案：等票 10 再測准入（本票驗收條件無法證明）；為了測試新增假的 Run 表（會與票 10 的資料模型衝突）。
- 影響：**票 10 的派工交易必須呼叫同一個 `admit`，並以 Task 需要的能力為輸入**，否則「未確認授權的 Workspace 無法建立 Run」只在端點成立。目前 `check_admission` 走唯讀連線；票 24（R1-a）需要把它移進寫入佇列的交易，才能與授權更新建立可查驗的持久化順序。
- 審閱：認同 2026-10-22

### AI-54：能力詞彙、兩個範本的內容與高風險能力的處理
- 重要度：高
- 時間／階段：2026-10-22，票 06 實作
- 決策：(1) 能力共 12 個：`read_files`、`write_files`、`web_read`、`run_build_test`、`install_project_dependencies`，加上七個「不在 P3」的高風險能力 `admin_install`、`global_install`、`remote_write`、`deploy`、`external_message`、`payment`、`production_data`。授權的一項是「能力 + 目標資源（至少一項）+ 限制」。(2) 研究／文件範本：讀取檔案、編輯檔案、讀取公開網頁（唯讀、不登入、不提交表單）；不含建置、測試與安裝。可信專案開發（P3）範本：讀取檔案、編輯檔案、建置與測試（目標為 Workspace 目錄，限制為僅執行專案內建置測試指令、不使用系統管理員身分）、安裝專案依賴（目標為專案設定檔與 lockfile 宣告的套件來源，限制為僅安裝到專案目錄、不做全域安裝、不使用管理員身分）。範本的目標資源代入該 Workspace 的目錄。(3) 沒有任何範本含高風險能力；使用者只能在編輯時明確加入，加入後確認摘要逐項標示「高風險」，且確認必須承認風險。(4) 不替使用者預選範本，也不依目錄是否為 Git repo 推測；兩個範本並列，也可以不套用範本自行組成。
- 理由：票與 ADR-0003 只列出類別（編輯、指定環境建置測試、指定來源安裝依賴）與必須另外授權的清單，沒有給出可程式化的詞彙；「讀取公開網頁」是研究／文件工作的合理基本能力，是我加入的，屬於範本內容而非產品需求。「指定環境」「指定來源」以目標資源與限制文字表達，目前不由 Runtime 強制，強制要到 Agent 與 permission-prompt（票 11、22）才有可執行的位置。
- 替代方案：研究範本只含檔案讀寫（Agent 無法上網查資料）；依 `.git` 是否存在預選開發範本（違反「不被強制套用開發範本」的精神，且 WorkspaceFs 要多一個 Port 方法）。
- 影響：能力與限制目前只是「被保存、被准入檢查」的資料；實際攔截屬後續票。新增能力要同時更新 Domain 的 `Capability::ALL`、`label`、`is_high_risk`、`runs_trusted_code` 與 Web UI 型別。
- 審閱：認同 2026-10-22

### AI-55：確認流程的細節（摘要、風險承認、內容未變、來源判定）
- 重要度：中
- 時間／階段：2026-10-22，票 06 實作
- 決策：(1) 新增預覽端點 `POST /api/v1/workspaces/{id}/authorization/summary`（只回傳摘要，不保存），摘要含逐項能力、目標、限制、高風險標記、必須揭露的風險與 `requires_risk_acknowledgement`；風險文字固定包含「專案與第三方依賴程式會以目前 Windows 使用者的身分執行」。(2) 授權內容含會執行專案或依賴程式的能力（建置測試、安裝依賴、管理員／全域安裝）或任何高風險能力時，`PUT /api/v1/workspaces/{id}/authorization` 必須帶 `acknowledged_risk: true`，否則 400 `risk_acknowledgement_required`；研究／文件範本不需要。承認旗標由 UI 的勾選產生，Runtime 無法證明使用者真的看過摘要。(3) 內容（排序與前後空白正規化後）與目前生效的 revision 相同時，不建立新 revision，回 200 與既有 revision；真正的新 revision 回 201。(4) 空授權（沒有任何能力）是合法的，等於撤回全部能力，會建立新 revision。(5) revision 的來源由 Runtime 判定，不採信用戶端：沒帶範本為 `custom`，帶範本且內容與該 Workspace 的範本完全一致為 `template`，否則 `template_edited`，並記錄 `base_revision`（前一個 revision）與 `confirmed_by: "desktop"`（只有桌面憑證能確認；Agent 憑證得到 403）。(6) 已封存的 Workspace 不能再修改授權（409 `workspace_archived`）。
- 理由：票要求確認摘要揭露風險、確認後才生效、每次修改有來源與時間；摘要預覽加上承認旗標是在 API 層讓「先看再確認」可被測試的最小做法。內容未變不建立 revision，避免重複點擊製造沒有意義的歷史，也符合「每次『修改』產生新 revision」。
- 替代方案：用摘要雜湊綁定確認請求（可證明確認的就是看過的內容，但需要保存摘要狀態，目前沒有需求）；內容未變也建立 revision（歷史充滿重複）；用戶端自報 `source`（可被偽造）。
- 影響：撤銷授權的介面、「仍有 N 個 Run 沿用舊授權」與 R1-a 順序屬票 24；本票的「修改」包含拿掉能力，但還沒有專門的撤銷流程。
- 審閱：未審

### AI-56：授權 revision 的儲存方式與事件
- 重要度：中
- 時間／階段：2026-10-22，票 06 實作
- 決策：migration 第 4 版 `authorization_revision(workspace_id, revision, grants, source, confirmed_at)`，主鍵為 `(workspace_id, revision)`，`grants` 與 `source` 存 JSON，時間為 UTC 毫秒；觸發器禁止更新與刪除（改授權只能追加新的 revision，目前生效的是最新一個）。同一個交易中追加事件 `authorization_confirmed`，聚合類型 `workspace_authorization`、聚合識別為 Workspace 識別、聚合版本等於 revision 編號，payload 含 grants 與 source。確認不更動 `workspace` 表（不增加 Workspace 的 `version`）。
- 理由：事件表本來就以（聚合類型、聚合識別、聚合版本）唯一，revision 編號剛好對應；不可變的 revision 讓之後「已准入的 Run 沿用當時的授權版本」（票 24）只需記下 revision 編號。授權不是 Workspace 狀態機的一部分，獨立聚合可避免每次改授權都改寫 Workspace 列。
- 替代方案：把授權塞進 `workspace` 表的 JSON 欄位（無歷史、無法追加式保存）；授權改動也遞增 Workspace 版本（兩個聚合版本互相牽動）。
- 影響：備份與還原（票 40）需涵蓋這張表；票 24 的准入以 `authorization_revision` 編號為依據。
- 審閱：未審

### AI-57：授權設定畫面的位置與操作方式
- 重要度：低
- 時間／階段：2026-10-22，票 06 實作
- 決策：授權設定畫面（`AuthorizationPanel`）放在主區上方；建立 Workspace 成功後自動開啟，既有 Workspace 在左側清單用「授權設定」按鈕重新開啟。編輯方式是每個能力一個核取方塊，勾選後出現「目標資源」與「限制」兩個文字區（每行一項）；套用範本只是把範本內容載入編輯區，之後仍可任意修改。必須先按「檢視確認摘要」才會出現「確認並保存授權」；編輯任何內容都會讓已看過的摘要與風險承認作廢。`scripts/smoke-desktop.mjs` 加入「在真實 App 內建立 Workspace、套用 P3、承認風險、確認保存，確認前後准入檢查結果不同」的檢查（同時驗證 Tauri 指令的參數對應）。
- 理由：D-21 的導覽與分頁結構尚未存在（AI-37），先確保功能可用，之後只需移動掛載位置；Tauri 預設把 JS 的 camelCase 參數對應到 Rust 的 snake_case，實際以冒煙測試確認。
- 替代方案：獨立的建立精靈步驟（D-20 的首次啟動精靈屬票 07）；用表格逐項編輯 targets（每行一項更簡單）。
- 影響：首次啟動精靈（票 07）可直接重用 `AuthorizationPanel` 作為「建立第一個 Workspace（授權範本確認）」步驟。
- 審閱：未審

### AI-52：票內平行改由 Worker 自開 subagent，同時最多 3 個
- 重要度：中
- 時間／階段：2026-10-22，票 05 結案後
- 決策：一張票仍由一個實作 run 負責；Worker 先定好介面與型別，再把彼此獨立、不改同一批檔案的部分交給 subagent 同時做，同時存在的 subagent 最多 3 個。從下一批新派的票開始套用，06、07 不重派。
- 理由：使用者希望單張票內部也能平行。subagent 用的是該 Work 帳號的額度，不會用到 Work-0；也不需要跨 worktree 合併。
- 替代方案：主控把一張票拆成多個 run、各自 worktree，再加規劃與整合 run（每張票多 2 個以上 run，合併衝突風險高）。
- 影響：單一帳號的額度消耗變快；AI-04「不使用內建 subagent」只限主控 session，不適用於 Worker。
- 審閱：認同 2026-10-22（使用者指定）

### AI-58：Goal 的最小形狀，與 Task 必須屬於一個 Goal
- 重要度：低
- 時間／階段：2026-10-22，票 09 實作
- 決策：Goal 只有標題（1–200 字元）與固定為 1 的版本，沒有修改或封存命令；每個 Task 必須屬於同一個 Workspace 的一個 Goal。工作面板建立 Task 時，可以選既有 Goal，或輸入新名稱（面板先建立 Goal 再建立 Task）。
- 理由：glossary 規定 Task 屬於 Goal；Controller 還不存在，使用者手動建立時需要一個最小的 Goal 入口。Goal 內容變更與「受影響 Task 重驗」屬於後續的票（Controller 對話、Gate 失效），不在本票。
- 替代方案：Task 的 goal_id 可為空：之後 Controller 建立的 Task 若沒有 Goal 會失去歸屬。也考慮過 Goal 一開始就有描述與狀態欄位：沒有使用情境，不做。
- 影響：之後新增 Goal 修改命令時 `version` 才會遞增；契約引用 `goal` 時目前只有版本 1 有效。
- 審閱：未審

### AI-59：契約欄位的完整性：「未知」不算完整，目標與完成條件不可標「不適用」
- 重要度：中
- 時間／階段：2026-10-22，票 09 實作
- 決策：六項契約欄位各有四種狀態：已填寫、不適用（必須附理由）、未知、未提供。只有「已填寫」與「不適用（有理由）」算完整；「未知」與「未提供」都不能 ready，但在缺項清單中分成 `field_unknown` 與 `field_missing`。`objective`（目標）與 `done_criteria`（完成與驗證條件）不接受「不適用」。填寫的文字是空白、或「不適用」沒有理由，儲存時就拒絕（400 `contract_invalid`），不當作未提供。
- 理由：ADR-0011 規定「未知事項標為未知，不能以空值、任意預設或假設冒充已確認」，且「契約不完整就不派工」；把「未知」當完整，等於讓假設通過准入。目標與完成條件是 Worker 做事與停手的依據，ADR 也要求 Worker 不能自行改寫完成標準，所以這兩項不可能「不適用」。
- 替代方案：把「未知」視為可派工、由 Worker 回報：較快，但違反 ADR 的後果段落（空欄位被當成完整）。允許所有欄位標不適用：使用者可用「不適用」繞過完成條件。
- 影響：要求使用者對真的不知道的事先補足或由 Controller 詢問，才能派工；這是刻意的保守。放寬規則時只需修改 `ContractFieldName::allows_not_applicable` 與 `admission_report`。
- 審閱：未審

### AI-60：契約引用的種類、儲存與失效判定
- 重要度：中
- 時間／階段：2026-10-22，票 09 實作
- 決策：引用目前只有三種可解析的種類：`task_contract`（同 Workspace 的 Task 與契約版本）、`goal`、`authorization`（目標是 Workspace 識別、版本是授權 revision）。儲存契約時只檢查形式（目標非空、版本 ≥ 1），不檢查目標是否存在；准入檢查時判定失效：目標不存在或不在同一個 Workspace、版本超過目前最新、被引用的 Task 已 `cancelled`。每個失效引用各列一個 `reference_invalid` 缺項並標明所在欄位。
- 理由：ADR 要求「保存可解析的引用與適用版本」，准入前檢查「引用可用性、版本」。成果、Stage、決策、知識等實體還不存在，無從解析，所以種類只涵蓋已存在的實體，後續的票加入新種類。儲存時不驗證，才能先存草稿再補被引用的內容，並讓「引用失效」成為准入檢查可觀察的結果。
- 替代方案：儲存時就拒絕失效的引用：草稿流程較不順，也看不到准入檢查拒絕的情境。接受任意字串當引用（例如檔案路徑）：Runtime 無法證明可解析，違反 ADR。
- 影響：使用者目前無法引用檔案或網址；需要時應等成果（票 12）與知識（票 37）落地後新增引用種類。
- 審閱：未審

### AI-61：准入檢查的範圍：契約、引用、授權，不含相依完成；所需能力放在契約內並隨版本保存
- 重要度：中
- 時間／階段：2026-10-22，票 09 實作
- 決策：進入 `ready` 的准入檢查包含：六項契約欄位、引用可用性與版本、Workspace 封存狀態、以及契約的 `required_capabilities` 與 Workspace 目前生效授權的比對（呼叫票 06 的 `authorization::admit`，不需要任何能力時不需要確認授權）。相依是否已完成不是 ready 的條件：面板以 `unfinished_dependencies` 顯示等待原因，Scheduler 在派工時才檢查。`required_capabilities` 是契約內容的一部分，隨契約版本保存。
- 理由：ticket 只列出「契約不完整、引用失效或授權不符」三類拒絕原因；ADR 的「相依」屬於「啟動准入」，而啟動（Run）要到票 10 才存在。若把相依完成納入 ready，新增相依就會讓 ready 的 Task 退回，狀態語意變得複雜。把所需能力放進契約，授權不符才有判定依據，也讓能力需求的變更留下版本歷史。
- 替代方案：把「相依已完成」納入 ready：pending/ready 的語意變成「等待相依」，與狀態表中的 pending 重疊。把所需能力放在 Task 上而非契約：變更不會有版本歷史。
- 影響：票 10／19 的派工交易必須自行檢查 `unfinished_dependencies` 為空，並重新呼叫 `admission_report`（授權可能在 ready 之後被撤銷，票 24）。
- 審閱：未審

### AI-62：契約變更的規則
- 重要度：中
- 時間／階段：2026-10-22，票 09 實作
- 決策：契約以整份取代；內容（含 `required_capabilities`，不含變更說明）與目前版本相同時不新增版本。版本只增不減，存在追加式資料表。`ready` 的 Task 契約變更後退回 `pending`，必須重新通過准入；其他非終止狀態保持原狀，`completed`、`cancelled` 不能改契約（409 `task_closed`）。不做樂觀鎖（沒有 expected_version）。
- 理由：ADR：「執行中契約變更須保存版本並依既有相依與 Gate 規則處理」，Gate 與 Run 都還沒有，本票只能做到保存版本、並避免已經 ready 的 Task 帶著舊的准入結果被派工。目前只有桌面 App 一個寫入者，不需要樂觀鎖。
- 替代方案：ready 的 Task 改契約後仍保持 ready：舊的准入結果會被沿用到新契約。加 expected_version：多使用者或 Controller 並行修改時才需要，屆時再加（票 17）。
- 影響：Controller（票 17）的 `update_task` 工具會同時修改契約；若出現並行修改，需補 expected_version。
- 審閱：未審

### AI-63：狀態轉換表與封存 Workspace 的處理
- 重要度：低
- 時間／階段：2026-10-22，票 09 實作
- 決策：完整的狀態轉換表集中在 `TaskStatus::allowed_next`（含 running、waiting_*、verifying、completed 等後續的票才會用到的轉換），本票只開放兩個命令：標記 ready（只從 pending）與取消（任何非終止狀態）。進入 ready 一律要通過准入檢查。Workspace 封存後，所有 Goal／Task 的變更命令（含取消）都被拒絕（409 `workspace_archived`），唯讀查詢仍可用；封存 Workspace 內的 Task 准入檢查回報 `workspace_archived`。
- 理由：狀態規則要集中在 Domain，後續的票只需新增命令；表格以 Domain 單元測試檢查（終止狀態沒有出口、非終止狀態都能取消、未通過准入不能 running、未驗證不能 completed）。封存 Workspace 的清理與取消語意不明確，先採最保守的全部拒絕。
- 替代方案：封存時自動取消所有 Task：會產生大量副作用，spec 沒有要求。
- 影響：封存後無法再取消殘留的 Task；若需要，由後續的 Workspace 生命週期票決定。
- 審閱：未審

### AI-64：相依只能新增、不能移除；事件掛在依賴者上
- 重要度：低
- 時間／階段：2026-10-22，票 09 實作
- 決策：本票只有新增相依：自我相依 400 `dependency_self`、造成循環 409 `dependency_cycle`（訊息含完整循環路徑）、重複新增冪等（200，不產生事件）、對方必須是同一個 Workspace 的 Task（否則 404 `task_not_found`）、依賴者已結束時 409 `task_closed`。`task_dependency_added` 事件掛在依賴者 Task 的聚合上並使其版本加一。相依對象已取消不會被拒絕（它只會一直留在未完成清單）。
- 理由：ticket 只要求「禁止自我依賴、檢查循環」，沒有要求移除；移除相依牽涉到 Gate 與重驗，留給有需要的票。
- 替代方案：同時提供移除端點：沒有驗收條件，且移除會引入「相依被移除後原本 blocked 的 Task 如何處理」的問題。
- 影響：使用者誤加相依時只能取消 Task 重建；面板沒有移除按鈕。
- 審閱：未審

### AI-65：工作面板的資料與畫面形狀
- 重要度：低
- 時間／階段：2026-10-22，票 09 實作
- 決策：`GET /workspaces/{id}/tasks` 一次回傳 Goal、每個 Task 的投影（目前契約、相依、未完成相依）與全部 12 個能力（供契約的「所需能力」複選）；只有 `pending` 的 Task 計算 `missing`（完整准入檢查），其他狀態一律是空陣列。面板依 9 個狀態分欄，入口是左側 Workspace 清單的「工作面板」按鈕；契約引用在畫面上以每行 `kind:target@version` 輸入。Task 識別以 `task_`、Goal 以 `goal_` 加 96 位元隨機值產生。
- 理由：ticket 要求「依狀態分欄顯示」與「面板顯示缺少項目」；缺項放在看板回應裡，面板不需要為每張卡再打一次准入檢查。非 pending 的 Task 已通過准入或不在准入範圍，不重算。引用輸入先用文字行，因為目前沒有可選的引用對象清單（成果、知識尚未存在）。
- 替代方案：每張卡都附缺項：非 pending 狀態的缺項沒有意義，且每次重載都重算授權。引用用下拉選單：可選對象的來源（跨 Task 契約版本）會讓畫面複雜很多。
- 影響：執行者、帳號、最後有效進度等卡片欄位（spec 使用者故事 28）要等 Run 與 checkpoint 存在後的票再加。
- 審閱：未審

### AI-66：Run 結束後 Task 的狀態：成功進 verifying，其餘一律回 ready
- 重要度：高
- 時間／階段：2026-10-22，票 10 實作
- 決策：Run `succeeded` 時 Task 從 `running` 推進到 `verifying`（`completed` 只能由後續票的驗證閘門設定，本票沒有任何路徑會把 Task 標為 `completed`）；Run `failed`、`interrupted`、`cancelled` 時 Task 退回 `ready`，使用者（或後續的 Scheduler）可以再派一個新的 Run。只有 Task 當下還是 `running` 才轉換：使用者已經取消的 Task 不會被 Run 結束拉回 `ready`。Task 沒有重試次數限制，預算（D-16）屬於票 36。
- 理由：票上要求「程序退出只產生 Run 結束事件，不把 Task 標為 completed」。狀態表中從 `running` 往前走只有 `verifying`，Worker 宣稱成功本來就該進驗證；失敗與中斷後最安全的是回到可再派的 `ready`，而不是自動重派（自動重派要有預算與退避，不在本票範圍）。
- 替代方案：成功後 Task 直接留在 `running`：Task 看起來還在執行，卻沒有任何寫入者，面板會誤導。成功後回 `ready`：Task 會被誤認為還沒做過，可能重複派工。
- 影響：`verifying` 的 Task 目前沒有任何出口（票 13 加入驗證閘門前，使用者只能取消它）；失敗後要靠使用者手動再按「派工」。
- 審閱：未審

### AI-67：結果不明的啟動不自動重送：找不到程序就中斷 Run
- 重要度：高
- 時間／階段：2026-10-22，票 10 實作
- 決策：Runtime 在「launch intent 已持久化、回執未寫回」時被重建（Run 停在 `starting`），Supervisor 以 `command_id` 呼叫 `ProcessControl::find`：找到就補寫回執、Run 進入 `running`；找不到（或程序已全部結束）就把 Run 標為 `interrupted`（原因 `launch_outcome_unknown`），釋放寫入所有權，Task 回 `ready`，**不自動重新啟動**。`find` 暫時出錯（Err）時維持 `starting`，下一次處理再試。
- 理由：「找不到」可能是「從未啟動」，也可能是「啟動後很快就結束」；兩者從外部無法區分，重啟可能讓同一件工作執行兩次（寫入衝突、重複外部操作）。spec 的原則是「先核對、再恢復」與「結果未知的外部操作不盲目重送」，由使用者重新派工才是安全的。
- 替代方案：找不到就直接重新 spawn：在「啟動後程序很快結束」的情況會重複執行。找不到就維持 `starting`：Task 永遠被卡住，沒有出口。
- 影響：崩潰發生在 spawn 之前時，使用者需要手動重新派工一次；程序若在 Runtime 崩潰期間自行結束，Run 也是 `interrupted` 而不是 `succeeded`（Adapter 的結束事件遺失，無從得知結果）。
- 審閱：未審

### AI-68：取消的兩階段與停止失敗的處理
- 重要度：中
- 時間／階段：2026-10-22，票 10 實作
- 決策：取消不新增 Run 狀態，而是記下 `cancel_requested_at`：尚未啟動（`queued`）的 Run 立即變 `cancelled`；已啟動的 Run 維持 `starting`／`running`，Supervisor 呼叫 `ProcessControl::stop`（寬限期預設 30 秒，D-11），回報 `StoppedGracefully` 或 `ForceKilled` 才完成取消、釋放 lease、Task 回 `ready`。`StillAlive` 或呼叫失敗時保持原狀（持有 lease），每次處理都重試，沒有次數上限。取消 Task 時，同一個交易會要求取消它進行中的 Run（Task 已經是 `cancelled`，Run 結束後不會把它拉回來）。Task 取消不會被進行中的 Run 擋住。
- 理由：使用者故事 43 要求舊 Worker 確認停止之前新 Worker 不能進入同一可寫目錄，所以 lease 必須等到確認停止才釋放；若在 `stop` 失敗時就當作取消完成，會產生兩個寫入者。沿用既有的七個 Run 狀態（票上列出的清單），避免新增第八個狀態。
- 替代方案：新增 `cancelling` 狀態：與票上的狀態清單不符，且 `cancel_requested_at` 已足夠表達。停止失敗幾次後放棄：放棄就等於釋放寫入所有權，風險更大；人工介入入口屬於裁定中心（票 22）。
- 影響：停不掉的程序會讓 Task 一直卡在 `running`；面板顯示「取消中，等待程序停止…」。
- 審閱：未審

### AI-69：Port 的調整：find、command_id、spawn_spec、Heartbeat 事件
- 重要度：中
- 時間／階段：2026-10-22，票 10 實作
- 決策：（1）`SpawnSpec` 新增 `command_id`；`ProcessControl` 新增 `find(&SpawnSpec) -> Option<Receipt>`（以 command_id 找回仍在受管的程序，同時讓 ProcessControl 之後能 `inspect`／`stop` 它）；`baton_ports::command_hash(&SpawnSpec)` 提供程序身分中的指令雜湊（SHA-256，program 與引數以 NUL 分隔，不含 command_id 與工作目錄）。（2）`RunSpec` 新增 `task_id`、`command_id`、`working_dir`；`AgentAdapter` 新增純函式 `spawn_spec(&RunSpec) -> SpawnSpec`（由 Adapter 決定啟動指令，Supervisor 持有程序）；`launch` 的語意改成「程序已啟動後建立事件來源，重複呼叫安全」，Runtime 重建後以它重新接上事件串流。（3）`AdapterEvent` 新增 `Heartbeat`，lease 只由 `Heartbeat` 續約（`Message` 等其他事件不續約）。（4）Supervisor 在 `stop`／`inspect` 之前，每個 Run 每個 Runtime 實例先 `find` 一次。
- 理由：spec 的 Port 草圖只有 `spawn/inspect/stop`，但「以 command_id 找回原執行」需要一個查詢；Adapter 與 ProcessControl 的分工（Adapter 組指令、ProcessControl 管程序）讓真實 Adapter（票 14）只需實作指令組裝與串流解析。heartbeat 的正式來源是 Worker 透過 Agent API／MCP 工具回報（票 11）；在那之前由 Adapter 事件代表，兩者呼叫同一個 Domain 命令 `Heartbeat`。
- 替代方案：把 command_id 放進 ProcessIdentity：偏離 spec D-10 的三元組。讓 Adapter 自己啟動程序：Supervisor 就無法統一做 Job Object 管理與核對。
- 影響：票 14（Claude Code Adapter）要實作 `spawn_spec`、並讓 `launch`／`events` 能在 Runtime 重建後重新接上同一個程序的輸出；票 11 的 `report_progress`／heartbeat 工具要呼叫 `RunCommand::Heartbeat`（帶 lease 世代）。
- 審閱：未審

### AI-70：lease 與過期核對的細節
- 重要度：中
- 時間／階段：2026-10-22，票 10 實作
- 決策：lease 有效期間 60 秒、heartbeat 間隔 15 秒（D-31）；派工、`BeginLaunch`、回執寫回與每次 heartbeat 都從當下重新計算 60 秒。只有已啟動（`starting`／`running`）且持有 lease 的 Run 才會在過期時被核對；`queued` 的 Run（還在等 Supervisor 處理）過期不核對。核對結果：`Alive` → 續約並追加 `run_lease_verified` 事件；`Dead` → 中斷 Run、釋放 lease、Task 回 `ready`；`Unknown` 或 `inspect` 出錯 → 不改變任何東西（下一次處理再核對一次）。過期的 lease 仍然接受 heartbeat（heartbeat 本身就證明存活）。heartbeat 只更新 lease 的兩個時間欄位，不追加事件。Worker 的回報（heartbeat、結束）帶 lease 世代，Run 已結束、lease 已釋放或世代不符一律以 `run_report_stale` 拒絕且不改變狀態。
- 理由：spec D-31「過期 lease 只觸發核對，不證明程序死亡」；事件表是追加式，每 15 秒追加一筆 heartbeat 事件會讓事件量膨脹，而 lease 時間欄位已足以追查。
- 替代方案：lease 過期就自動中斷：違反 D-31。每個 heartbeat 都記事件：事件量太大，價值有限。
- 影響：Worker 的存活歷史不在事件表；核對 `Unknown` 時沒有任何事件，只有日誌。
- 審閱：未審

### AI-71：Supervisor 的驅動方式與測試時序
- 重要度：中
- 時間／階段：2026-10-22，票 10 實作
- 決策：Supervisor 的工作都在 `tick` 裡完成（啟動 Outbox → 收事件 → 取消 → 核對 lease），每一步都冪等且只信任資料庫；事件串流只處理「已經到達」的事件（`now_or_never`），不等待。正式執行時由背景迴圈每秒或在派工／取消時被喚醒呼叫 `tick`；`RuntimeOptions { background_supervisor: false }` 不啟動背景迴圈，Harness 預設如此，測試以 `harness.supervise()` 決定何時處理，搭配 fake Clock。`RuntimeOptions::stop_grace` 預設 30 秒，測試可縮短。事件串流在 Runtime 重建後由 `AgentAdapter::launch` 重新接上。Adapter 事件的延遲在正式執行時最長約 1 秒。
- 理由：spec 要求測試不使用真實 sleep、可在任何邊界注入崩潰；若 Supervisor 在背景自行運轉，測試就無法決定性地重現「回執未寫回就重建」。崩潰注入以 fake ProcessControl 的 `hang_next_spawn_at` 讓 `spawn` 在指定點永遠不回來，測試丟棄該次 `tick` 即等同崩潰。
- 替代方案：每個 Run 一個常駐 tokio 任務：重建後要自行恢復任務集合，測試時序不可控。
- 影響：測試的「崩潰」以丟棄 future 模擬，只能在 `await` 點崩潰（與真實程序被殺相比，少了「寫到一半的交易」，但 SQLite 交易本身是原子的）。
- 審閱：未審

### AI-72：真實 ProcessControl 的設計取捨
- 重要度：中
- 時間／階段：2026-10-22，票 10 實作
- 決策：`WindowsProcessControl`：每個 Run 一個具名 Job Object `Local\Baton-Run-{command_id}`；不設 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`（Runtime 崩潰時程序繼續活著，之後由 `find` 找回）；程序先 `CREATE_SUSPENDED`、放進 Job 後才 resume，且 Job 已存在（同一個 command_id 重複 spawn）時在建立程序之前就拒絕；子孫不允許 breakaway；stdin／stdout／stderr 先導向 NUL（串流解析屬票 14）；`stop` 先在寬限期內輪詢等程序自行結束（目前沒有送出「請保存進度」的訊號，那屬於票 14／25 的受控停止），逾時以 `TerminateJobObject` 終止整棵樹並等最多 10 秒確認；`inspect` 以根程序為準（pid + 建立時間吻合且程序未結束為 `Alive`），沒有登記 handle 時退回以 pid 查詢，無法判定回 `Unknown`；command_id 不可含 `\` 或 NUL。
- 理由：D-10 與票上的驗收條件；不殺程序的 Job 設定讓「Runtime 崩潰後找回原執行」成為可能（存活 Run 的完整接管仍不在範圍內）。
- 替代方案：設 KILL_ON_JOB_CLOSE：Runtime 一崩潰所有 Worker 都被殺，使用者故事 75／恢復都無從談起。
- 影響：Windows 版本與工作階段相關的細節（Job 名稱的 `Local\` 命名空間、跨登入工作階段）是 Validation Required；30 秒寬限期在沒有受控停止訊號時等於「固定等 30 秒才強制終止」。
- 審閱：未審

### AI-73：派工的提示與工作目錄暫時做法；正式執行檔尚無 Adapter
- 重要度：低
- 時間／階段：2026-10-22，票 10 實作
- 決策：派工交易把「Worker 提示」（Task 標題加六項契約欄位的文字）與工作目錄（Workspace 路徑）固定在派工 Outbox 的內容裡，Runtime 重建後據此重組出相同的啟動指令；Context Builder（票 39）與 worktree（票 20）加入後取代這兩項。正式執行檔（`production_ports`）的 ProcessControl 在 Windows 上已是真實實作，但 AgentAdapter 仍是「尚未實作」（票 14），所以在正式執行檔中派工後 Run 會以 `launch_failed` 結束，Task 回到 `ready`。
- 理由：票 10 的驗收以 fake Adapter 進行；真實 Adapter 屬票 14。把內容固定在 Outbox 可以保證重建前後的 command_hash 一致，`find` 才找得回同一個程序。
- 替代方案：重建時再從 Task 契約重算提示：契約可能在派工後被改版，指令雜湊會變。
- 影響：使用者在正式執行檔按下「派工」目前只會看到 Run 失敗並附原因；票 14 完成後才會真的執行。
- 審閱：未審

### AI-74：Run 的資料模型與 API／面板形狀
- 重要度：低
- 時間／階段：2026-10-22，票 10 實作
- 決策：新增資料表 `run`（含 launch intent 的 `command_id`、程序身分三欄、`cancel_requested_at`、`end_reason`）、`execution_lease`（`run_id` 主鍵，`task_id` 上有「尚未釋放者唯一」的部分唯一索引）與 `outbox`（`(kind, run_id)` 唯一，`delivered_at` 為空代表待處理）。Run 識別以 `run_` 加 96 位元隨機值、command_id 以 `cmd_` 加 96 位元隨機值產生。API：`POST …/tasks/{task_id}/dispatch`（201 回 `queued` 的 Run）、`GET …/runs`、`GET …/runs/{run_id}`、`POST …/runs/{run_id}/cancel`（200，冪等）；`RunView` 是 Run 欄位展平加上 `lease`；工作面板 `GET …/tasks` 新增 `runs`（該 Workspace 全部 Run，依建立順序）。面板：ready 的卡片有「派工」，卡片顯示該 Task 世代最大的 Run 的狀態列，進行中的 Run 有「取消 Run」，已要求取消則顯示「取消中，等待程序停止…」。性質測試使用 `proptest`（新增為 `baton-domain` 的 dev-dependency）。
- 理由：沿用票 09 的形狀（路徑、錯誤代碼、`TaskApiError`）；唯一索引讓「同一 Task 最多一個有效寫入所有權」不只靠應用程式邏輯；`Outbox` 以 `delivered_at` 表示已處理，保留紀錄方便追查。
- 替代方案：Run 結束後刪除 Outbox 項目：失去追查紀錄，且 Supervisor 重建時無法重組啟動指令。
- 影響：Run 與事件目前只增不減，沒有清理；面板的 Run 資料隨 Workspace 增長（歷史多了以後要分頁，屬後續票）。
- 審閱：未審

### AI-75：平行票合併衝突以整合 run 處理
- 重要度：中
- 時間／階段：2026-10-22，票 07、10 合併
- 決策：兩張平行開發的票改到同一批檔案時，先通過驗證的票先合併；另一張停止進行中的驗證，改派一個整合 run（`.orchestration/integrate.template.md`）合併最新的 `rebuild/v1`、解衝突、順延 migration 與 AI 編號，再重新驗證。整合 run 不計入修正次數。
- 理由：票 07 與票 10 同時改了 Store、Ports、API、Tauri 指令與前端用戶端共 15 個檔案，兩邊都新增 migration 6 與 AI-66 起的條目。在舊基底上驗證通過的結果，合併後不再成立。
- 替代方案：由主控自己解衝突（會用到主帳號額度，且衝突涉及程式邏輯）；或讓平行票改為循序（降低並行度）。
- 影響：每遇到一次衝突多一個整合 run 與一個驗證 run。被停止的驗證 run 在 runs.md 記為「中止」。
- 審閱：未審

### AI-76：Profile 的「預期身分」怎麼決定，以及不符時怎麼處理
- 重要度：高
- 時間／階段：2026-10-22，票 07 實作
- 決策：票面要求「實際認證身分與 Profile 預期不符時標示並阻擋」，但沒有說預期身分從哪來。採兩條路：(1) 建立 Profile 時可選填 `expected_account`（電子郵件），第一次核對起就比對；(2) 沒填時，第一次在該 Profile 自己的原生登入（`authMethod=claude.ai`）且設定目錄相符時核對到的帳號，固定為預期身分（追加 `account_profile_identity_pinned` 事件）。之後核對到不同帳號（不分大小寫）就標 `identity_mismatch` 並阻擋，不會悄悄改寫預期身分。認證來源被覆蓋時回報的帳號一律不採用、也不固定。另外，同一個帳號被兩個使用中的 Profile 認到時，較晚建立的標 `duplicate_identity` 並阻擋。登入了但取不到帳號（`email` 缺漏）標 `identity_unconfirmed` 並阻擋。
- 理由：沒有預期身分就沒有「不符」可言；讓使用者必填會增加首次啟動負擔，所以預設用「第一次原生登入的帳號」固定。同帳號綁兩個 Profile 會讓額度與並行判斷失真，且正是「以為在用 A、實際在用 B」的常見成因（兩個目錄都登入同一人）。
- 替代方案：預期帳號必填（較嚴但多一個欄位）；不固定、只比對覆蓋來源（抓不到「在終端機改登入別的帳號」）；允許同帳號多個 Profile（額度重複計算）。
- 影響：使用者在 Profile 的終端機改登入另一個帳號後，該 Profile 會被阻擋且目前沒有「接受新身分」的操作，需封存後重建（會是新的設定目錄與新的登入）。若需要「接受新身分」，在帳號頁加一個明確動作與命令即可。
- 審閱：未審

### AI-77：「阻擋派工」以 `dispatchable=false` 加 `blockers` 表示
- 重要度：高
- 時間／階段：2026-10-22，票 07 實作
- 決策：Scheduler 與派工尚未存在，所以無法測「實際不被派工」。Domain 的 `profile::blockers`／`views` 判定每個 Profile 是否可派工，`GET /api/v1/profiles` 與精靈都回傳 `dispatchable` 與逐項 `blockers`（穩定代碼：`profile_unverified`、`profile_not_logged_in`、`auth_source_overridden`、`config_dir_mismatch`、`identity_unconfirmed`、`identity_mismatch`、`duplicate_identity`、`profile_archived`）。尚未核對過的 Profile 也不可派工。
- 理由：與 AI-35 相同的處理方式：現在能被外部觀察的只有 API 回應；判定集中在 Domain，後續票不必自行比對。
- 替代方案：先做一個假的派工端點來測（超出本票範圍）。
- 影響：Scheduler／Run 准入的票必須只選 `dispatchable` 的 Profile，並補上「被阻擋的 Profile 沒有被派工」的情境測試。核對結果沒有時效：目前不會因為太久沒核對而失效，是否需要定期重新核對由額度觀測或准入的票決定。
- 審閱：未審

### AI-78：認證來源的判定依據（實測 Claude Code 2.1.307）與環境變數處理
- 重要度：高
- 時間／階段：2026-10-22，票 07 實作
- 決策：以 `claude auth status --json`（`CLAUDE_CONFIG_DIR` 設為該 Profile 目錄）判定。本機實測輸出：原生登入 `authMethod: "claude.ai"`（含 `email`、`configDirectory`）；環境變數 `ANTHROPIC_API_KEY` → `"api_key"` 加 `apiKeySource: "ANTHROPIC_API_KEY"`（沒有 `email`）；`ANTHROPIC_AUTH_TOKEN` 或 `CLAUDE_CODE_OAUTH_TOKEN` → `"oauth_token"`；未登入 → `loggedIn: false`、結束碼 1 但仍輸出 JSON。判定規則：只有 `claude.ai` 且 `apiProvider` 是 `firstParty` 才算 `NativeLogin`；`api_key`、`oauth_token`、非 firstParty 供應商與不認識的方式全部視為覆蓋並阻擋。另外比對 CLI 回報的 `configDirectory` 與 Profile 目錄（不分大小寫、斜線、`\\?\` 前綴）。核對**沿用 Runtime 的環境變數**，只覆寫 `CLAUDE_CONFIG_DIR`，不像 AI-02 那樣清除。
- 理由：核對的目的就是看出覆蓋；清除環境變數會讓覆蓋看不見。這樣得到的是「以 Runtime 目前環境啟動 Claude Code 時實際會用什麼」，之後 Adapter 若啟動前清除環境，只會更嚴格、不會更寬鬆。`apiKeySource` 的 `apiKeyHelper` 值只是依字面推測，沒有實測（本機沒有設定 helper）。
- 替代方案：只看電子郵件有沒有變（抓不到沒有 email 的 API key 覆蓋）；讀取 `.credentials.json` 判定（要碰登入資料，違反驗收條件）。
- 影響：使用 `claude auth login --console`（Console／API 計費）登入的 Profile，若 CLI 回報的 `authMethod` 不是 `claude.ai` 會被視為覆蓋而阻擋，需要實測確認；CLI 改版改了 `auth status` 的輸出格式時，核對會回報 `profile_check_failed`。正式實作已用本機真實的 `claude`（空目錄、已登入的 `.claude-Work-3`）以 `--ignored` 的 live 測試與桌面冒煙測試驗證。
- 審閱：未審

### AI-79：新增 ClaudeEnvironment Port，並為 WorkspaceFs 加 `ensure_directory`
- 重要度：中
- 時間／階段：2026-10-22，票 07 實作
- 決策：新增 `ClaudeEnvironment` Port（`probe_claude`、`probe_git`、`auth_status`、`open_login_terminal`）。`Ports` 結構多一個 `claude_env` 欄位；harness 提供 `FakeClaudeEnvironment`（可腳本化工具存在與版本、各設定目錄的原生登入、環境變數覆蓋、登入終端機，並像真實 Claude Code 一樣把登入資料寫進該 Profile 目錄）。Port 沒有任何下載或安裝動作。`WorkspaceFs` 新增 `ensure_directory`，用於建立 Profile 的設定目錄。
- 理由：spec 的 Port 草圖沒有涵蓋「執行 claude／git 探測」與「開終端機」，這些都是外部副作用，依 D-04 必須是 Port。「不執行任何下載腳本」因此可以用「Port 上沒有這個能力」加上測試斷言「沒有 spawn」來證明。
- 替代方案：用既有的 `ProcessControl.spawn`（它沒有環境變數與輸出擷取，語意也是 Run 的程序）。
- 影響：後續建立 `Ports` 的程式要補上 `claude_env`。真實 Claude Code Adapter（票之後）可以共用 `ClaudeCli` 的探測。
- 審閱：未審

### AI-80：Profile 設定目錄的位置、封存與資料表
- 重要度：中
- 時間／階段：2026-10-22，票 07 實作
- 決策：設定目錄固定在 `<資料目錄>\profiles\<profile id>`（id 為 `prof_` 加 96 位元隨機值），由 Runtime 建立，不讓使用者自選；Domain 另外保證任兩個 Profile（含已封存者）的目錄不相同也不互相包含。新增 `POST /api/v1/profiles/{id}/archive`（票面沒有明列）：D-32 的 8 個上限若沒有釋出名額的方式，使用者只能永久佔滿；封存不刪除目錄（登入資料仍在那裡，刪除是不可逆動作）。Profile 名稱必填、使用中不可重複（不分大小寫）、50 字元以內。新增 migration 7（接在票 06、09、10 的 migration 之後）：`account_profile`（核對結果存成 JSON 欄位）與 `setup_state`；所有變更與事件在同一個交易追加。
- 理由：D-17 規定 Profile 原生設定目錄放在資料目錄之下；封存與刪除目錄分開處理比較安全。
- 替代方案：讓使用者選任意目錄（可能共用或指到自己平常的 `~/.claude`，違反「互不共用」）；不做封存（8 個上限變成陷阱）。
- 影響：被封存 Profile 的資料夾會留在磁碟上，清理（含登出）需要另一個明確的功能。使用者原本想沿用的現有 `.claude-Work-n` 目錄不能直接匯入，要在新的 Profile 目錄重新登入。
- 審閱：未審

### AI-81：登入終端機的開啟方式
- 重要度：中
- 時間／階段：2026-10-22，票 07 實作
- 決策：App 呼叫 Runtime 的 `POST /api/v1/profiles/{id}/login-terminal`，由 Runtime 執行 `cmd /d /c start "Baton - Claude login" cmd.exe /k claude auth login`，環境變數 `CLAUDE_CONFIG_DIR` 設為該 Profile 目錄，視窗以 `/k` 保持開啟。Runtime 的標準輸入輸出是空的，直接用 `CREATE_NEW_CONSOLE` 會讓新視窗的 `cmd` 讀到 EOF 立刻結束，所以透過 `start` 讓新視窗取得自己的主控台。登入完成後由使用者按「重新核對」，Runtime 不輪詢登入結果。
- 理由：票面要求「可在 App 開啟的終端機中以該 Profile 執行原生登入」。只執行 `claude auth login`，不碰登入資料，也不下載任何東西。
- 替代方案：Tauri 殼自己開終端機（登入動作會從 Runtime 搬到殼，harness 無法測）；內嵌終端機元件（超出範圍）。
- 影響：自動化測試只驗證「為選定 Profile 呼叫 Port、使用它自己的設定目錄」與命令列內容（`login_terminal_command` 的單元測試）；真實視窗是否出現、`claude auth login` 是否在其中正常完成沒有自動化覆蓋，屬 **Validation Required**，已寫入 `docs/development.md`，列入票 51 的平台驗收較妥。`start` 的視窗標題刻意用 ASCII，避免主控台字碼頁亂碼。
- 審閱：未審

### AI-82：Claude Code 最低版本與 Git 的檢查標準
- 重要度：中
- 時間／階段：2026-10-22，票 07 實作
- 決策：最低 Claude Code 版本暫定 2.1.0（`MIN_CLAUDE_CODE_VERSION`）；低於它標為 `too_old`，並提示 `claude update`。版本取不到或探測失敗標為 `unknown`（不當作可用）。Git for Windows 只檢查在 PATH 上（`where git`），不設版本下限，也不判斷是不是 Git for Windows 發行版。
- 理由：spec D-20 說最低版本在 Adapter 驗證時固定；目前實測過的是 2.1.x 系列，且 `claude auth status` 存在於這個系列。官方文件把 Git for Windows 列為建議而非必要（沒有它 Claude Code 改用 PowerShell 工具），但 D-20 與 worktree 管理需要 git，所以仍檢查並提示。
- 替代方案：不設最低版本（無法呈現「版本過舊」）；把缺少 Git 視為阻擋（官方並不要求）。
- 影響：Adapter 驗證票確認實際需要的最低版本後，改 `baton-domain/src/setup.rs` 的常數與測試。Git 缺少目前只是提示，不阻擋建立 Profile 或派工。
- 審閱：未審

### AI-83：精靈顯示的官方指令與其核對狀況
- 重要度：低
- 時間／階段：2026-10-22，票 07 實作
- 決策：Claude Code 缺項顯示 `irm https://claude.ai/install.ps1 | iex` 與 `winget install Anthropic.ClaudeCode`；過舊顯示 `claude update`；Git 缺項顯示 `winget install --id Git.Git -e --source winget`，並附說明頁網址（Claude Code：code.claude.com/docs/en/setup；Git：git-scm.com/download/win）。Claude Code 的三條指令已對照官方安裝文件（2026-10-22 查閱）；Git 的 winget 指令沿用 Git for Windows 官方下載頁慣用的 `Git.Git` 套件識別，沒有逐字核對官網。指令只是文字，UI 沒有執行按鈕，說明頁網址以純文字顯示（不做成連結，避免 WebView 被導走）。
- 理由：D-20 規定缺項只引導、不代為安裝。
- 替代方案：顯示可點的連結（需要 Tauri 的 opener 外掛與權限）。
- 影響：官方指令改版時需更新 `setup.rs`。
- 審閱：未審

### AI-84：核對失敗與「重新檢查」的語意
- 重要度：低
- 時間／階段：2026-10-22，票 07 實作
- 決策：單一 Profile 核對本身失敗（claude 沒回應、輸出不是預期的 JSON）時回 502 `profile_check_failed`，不改寫該 Profile 已記錄的結果。精靈的 `GET /api/v1/setup` 每次都重新探測 Claude Code 與 Git、讀取已記錄的 Profile 結果；`POST /api/v1/setup/recheck`（「重新檢查」）另外並行重新核對每個使用中的 Profile，個別失敗者略過、保留先前結果。工具探測失敗回報 `unknown` 與原因，不讓精靈整體失敗。RuntimeClient 對這幾個會執行 `claude`／`git` 的請求等待上限放寬為 60 秒（一般請求維持 2 秒）。
- 理由：核對失敗不該被誤記成「未登入」而覆蓋掉先前可信的結果；也不該因一個 Profile 卡住而讓整個精靈沒有回應。
- 替代方案：失敗時把結果記為「未知」並阻擋（較保守，但暫時性的 CLI 逾時會讓所有帳號被阻擋）。
- 影響：Claude Code 長時間不可用時，舊的「可派工」結果會一直保留；派工前的准入應自行重新核對（見 AI-77）。
- 審閱：未審

### AI-85：精靈狀態、背景行為告知的保存與 UI 位置
- 重要度：低
- 時間／階段：2026-10-22，票 07 實作
- 決策：`ready` 的定義是 Claude Code 與 Git 都 `ok`，且至少有一個可派工的 Profile；背景行為告知是否確認另外以 `setup_state` 表保存（首次確認追加 `background_notice_acknowledged` 事件，之後重複確認不改變時間也不追加事件）。UI 在 App 主區依序放首次設定精靈、帳號頁、平台狀態頁（導覽尚未存在，與 AI-37 相同）；全部完成且已確認告知時精靈縮成一行「首次設定已完成」加「重新檢查」。已有 8 個 Profile 時，新增按鈕停用並說明上限（Runtime 端仍會拒絕第 9 個）。精靈步驟中的「匯入共同設定」與「建立第一個 Workspace（授權範本確認）」不在本票範圍，由後續的票加入。
- 理由：票面只要求精靈檢查三項環境與告知背景行為。
- 替代方案：把告知確認存在 WebView 的 localStorage（重建 Runtime 或換視窗資料夾後會重複顯示，也不在事件紀錄中）。
- 影響：加入導覽的票只需移動這兩個元件的掛載位置。
- 審閱：未審

### AI-86：票 07 整合 rebuild/v1：migration 順序、AI 編號與文件段落
- 重要度：中
- 時間／階段：2026-10-22，票 07 整合（R021-07-integrate）
- 決策：(1) `MIGRATIONS` 依索引計版本，整合分支已有 `run_lease_outbox`（第 6 版），本票的 `account_profile_and_setup_state` 順延為第 7 版；兩個 migration 都完整保留，兩邊的資料表都會建立（AI-80 的 migration 編號已同步改為 7）。(2) 本票原本的 AI-66～AI-75 與整合分支的票 10 條目重號，順延為 AI-76～AI-85；整合分支的條目沒有改動，程式碼與其他文件沒有引用這些編號，只有 AI-84 內文的交互引用同步改成 AI-77。(3) `ApiState`、`Ports`、前端 `AppClient` 兩邊的新欄位與方法全部保留（`claude_env`、`supervisor_wake`；`AccountClient` 與 `TaskClient.dispatchTask`／`cancelRun`），票 10 的 Runtime 整合測試 `supervisor_process_tree.rs` 補上 `claude_env: ClaudeCli`；`docs/development.md` 兩邊新增的段落與表格列都保留。
- 理由：兩邊的 migration 彼此獨立（沒有共用資料表），順序只影響版本號；已發布的 migration 不可修改，所以順延尚未發布的本票版本。
- 替代方案：把本票的 migration 插在票 10 之前（會改動整合分支已有的版本號，不可行）。
- 影響：開發中的本機資料庫若已套用過舊版本號的 migration，需要重建；尚未有任何發布版本。
- 審閱：未審


### AI-87：Agent 憑證的簽發時機、交付方式與有效性
- 重要度：高
- 時間／階段：2026-10-22，票 11 實作
- 決策：(1) **准入 = Supervisor 的啟動步驟**：`BeginLaunch` 持久化成功之後、`spawn` 之前，為該 Run 簽發 Agent 憑證（`CredentialIssuer::issue_agent`），綁 Workspace、Run 與世代（`credential` 表以第 8 個 migration 加三欄（`agent_report` 是第 9 個；第 7 個已由票 07 的 account_profile 使用），`run_id` 有唯一索引：每個 Run 最多一個憑證）。簽發失敗就不啟動，下一次 tick 由既有的「找不到程序就中斷」收尾。(2) 憑證**只經 `SpawnSpec.env`** 的環境變數 `BATON_AGENT_TOKEN`（連同 `BATON_RUNTIME_URL`，值是 `http://127.0.0.1:<實際埠>`）交給程序；Adapter 的 `spawn_spec` 仍是純函式、拿不到憑證，由 Supervisor 在 Adapter 的結果上附加環境變數。`SpawnSpec` 的 `Debug` 只印環境變數名稱，環境變數不納入 `command_hash`（所以 Runtime 重建後以 command_id 找回原程序不需要憑證）。真實 `WindowsProcessControl` 以 Unicode 環境區塊傳入（Runtime 自己的環境加上 `env`，同名以 `env` 為準），已用真實程序測試。(3) **沒有時間上限**（`expires_at = NULL`）：有效與否取決於「Run 仍是寫入所有者」（見 AI-89），Run 可能跑好幾小時，固定 TTL 會讓長 Run 中途失效；Runtime 重啟不輪替 Agent 憑證（Worker 程序在 Runtime 崩潰時仍活著，AI-72），只輪替桌面憑證。(4) MCP 設定檔由 `baton_domain::agent::mcp_config_json` 產生，只含 shim 指令與 `${BATON_AGENT_TOKEN}`／`${BATON_RUNTIME_URL}` 參照，真實 Adapter（票 14）直接使用。
- 理由：D-22 要求「Run 准入時簽發、只經環境變數交給 MCP shim」。准入的外部可觀察點就是 Run 被啟動的那一刻；放在 spawn 之前才能保證程序一出生就有憑證。把憑證放進 `SpawnSpec.env` 讓 fake ProcessControl 能記錄它，harness 因此能取得與真實 Worker 相同的憑證做測試。
- 替代方案：派工（`Dispatch`）時簽發（Run 還在 `queued`，Runtime 重啟後明文已遺失，必須重簽）；憑證放進 MCP 設定檔的 `env`（違反「設定檔不含憑證」）；用 TTL（長 Run 失效）。
- 影響：**Validation Required（票 14 以真實 Claude Code 驗證）**：(a) Claude Code 是否對 MCP 設定中 `env` 值展開 `${VAR}`，以及 stdio MCP server 是否繼承 `claude` 程序的環境（若不繼承或不展開，改由設定檔以外的管道交付，但仍不得寫入設定檔）；(b) Worker 的 Bash 工具子程序也會繼承 `BATON_AGENT_TOKEN`，模型若執行 `echo` 會讓憑證進入 transcript。這個憑證只能碰自己的 Run、Run 結束即失效，影響有界，但 transcript 匯入時的秘密遮蔽（D-22 最後一句）需要涵蓋它。shim 執行檔的位置與打包（sidecar）不在本票，屬票 14、34。
- 審閱：未審

### AI-88：Agent API 的形狀、角色分流與「不信任請求內的 Workspace／Run」
- 重要度：高
- 時間／階段：2026-10-22，票 11 實作
- 決策：(1) 每個工具一個 `POST /agent/v1/tools/{tool}`（`get_task_contract`、`report_progress`、`save_checkpoint`、`report_blocked`），一律 POST＋JSON，路徑不含任何 Workspace 或 Run 識別；shim 只需「工具名稱 → 路徑、`arguments` → body」。(2) `guard` 依路徑前綴 `/agent/v1/` 決定所需角色：該前綴之下只接受 Agent 憑證，其餘（含未知路徑）只接受桌面憑證；角色不符一律 403 `role_not_allowed`，Agent 憑證沒有範圍則 401 `credential_invalid`。通過驗證的憑證記錄放進請求的 extensions，處理函式不必再查一次。(3) 身分與歸屬**完全取自憑證**（Workspace、Run、世代）。請求可以（但不需要）在 body 帶 `workspace_id`／`run_id`，只要和憑證的範圍不一致就 403 `agent_scope_mismatch`，不會改用請求裡的值，也不會碰到對方的資料；順序是先比對歸屬、再驗證內容、最後才讀寫。(4) shim 的請求沿用 `X-Baton-Client: baton-desktop` 這個固定標頭值（它的用途是讓瀏覽器的跨站請求觸發預檢而被拒絕，與呼叫者角色無關）。
- 理由：「Agent 存取其他 Workspace 或 Run 的資料被拒絕」需要一個可被測試的拒絕點。路徑不帶識別是最強的做法（沒有可竄改的識別），但 Worker 可能被提示注入而主動填寫別人的識別，所以對「聲稱不一致」明確回報而不是靜默忽略，讓越權嘗試可見、可被日後的稽核使用。
- 替代方案：路徑帶 `{workspace_id}/{run_id}` 再核對憑證（識別重複出現在兩處、容易讓實作誤用路徑值）；為 shim 另開一個標頭值（增加一個沒有安全意義的差異）；把管理 API 與 Agent API 拆成兩個埠（違反 D-23 的單一既定埠）。
- 影響：之後新增的 Agent 工具（`register_artifact`、`request_decision`、`send_message`…）都放在同一個前綴下並沿用同一套範圍與歸屬檢查；Controller 專屬工具的憑證角色（目前只有 `desktop`／`agent` 兩種）由票 16、17 決定。越權嘗試目前只在日誌留下一行「方法、路由樣板、狀態碼」，沒有寫入稽核表。
- 審閱：未審

### AI-89：Run 結束或被接替後的憑證：讀取被拒、回報被拒但留稽核
- 重要度：高
- 時間／階段：2026-10-22，票 11 實作
- 決策：憑證列**不刪除**，有效性每次請求即時判定：Run 尚未結束、仍持有 execution lease、且憑證的世代等於 Run 與 lease 的世代，才是目前的寫入所有者（`baton_domain::run::is_current_writer`，Domain 的過期世代檢查與 API 共用這一個判定）。不是目前寫入所有者時：`get_task_contract` 回 401 `credential_revoked`；三個回報工具回 409 `run_report_stale`，Domain 不改變任何狀態，但同一個交易會把回報內容追加到 `agent_report`（`accepted = 0`，追加式，觸發器禁止更新與刪除）。面板上的最新進度／checkpoint／受阻只取 `accepted = 1` 的最後一筆，所以過期回報不會覆蓋它。
- 理由：票要求「過期世代的回報被保留為稽核但不改變目前狀態」。若 Run 結束時就刪除憑證，遲到的回報會變成 401「不認識的憑證」，無法知道是誰、說了什麼；保留憑證列才能把它歸屬到正確的 Run 與世代。讀取不需要稽核，直接拒絕最單純；回報則可能含有有價值的遲到結果（例如被判死但其實還活著的 Worker 的最後 checkpoint），保留供人工查看。
- 替代方案：Run 結束時刪除或標記撤銷憑證（遲到的回報無法稽核）；遲到的回報也套用到目前狀態（違反「世代過期不改變狀態」）；讀取也留稽核（沒有需求，讀取不改變任何東西）。
- 影響：稽核內容可能含有 Worker 自行填寫的任何文字（包括它自己的憑證，如果它把環境變數印出來並放進回報）：目前沒有秘密遮蔽，屬 D-22「已知秘密樣式遮蔽」尚未實作的範圍。`agent_report` 沒有清理機制（與事件表相同），備份與還原（票 40）需涵蓋它。Run 被取消時舊程序若停不掉（`StillAlive`），該 Run 仍持有 lease，它的憑證仍是目前寫入所有者，這與「寫入所有權要等確認停止才釋放」一致。
- 審閱：未審

### AI-90：`report_blocked` 讓 Task 進入 `blocked`，Run 不結束
- 重要度：中
- 時間／階段：2026-10-22，票 11 實作
- 決策：被接受的 `report_blocked` 讓 Task 從 `running` 進入 `blocked`（只在 Task 當下確實是 `running`；已被使用者取消或已離開 `running` 的 Task 不動），並在 Run 上記錄受阻原因。**Run 本身不因此結束**：它仍是 `running`、持有 lease，直到 Worker 程序結束；之後 Run 的結束只在 Task 還是 `running` 時才轉換 Task，所以受阻的 Task 不會被 `succeeded` 拉到 `verifying`，也不會被 `failed` 退回 `ready`。
- 理由：D-08 與 D-07 描述的 Worker 流程是「保存 checkpoint，回報受阻，之後結束」；若 Task 仍停在 `running`，成功結束會被誤判成可驗證的成果。`blocked` 已在狀態表中（`Running → Blocked`），不需要新增狀態。
- 替代方案：只記錄、不改 Task 狀態（Worker 以成功結束時，Task 會進入 `verifying`，把受阻當成完成）；Run 同時結束（Worker 還要做收尾，且 Domain 沒有「Worker 自述結束」之外的結束路徑）。
- 影響：目前 `blocked` 的 Task 沒有 UI 出口（只能取消）；解除阻擋、建立裁定請求與重派屬票 22、36。`blocked` 欄位已存在於工作面板的狀態分欄。
- 審閱：未審

### AI-91：Worker 回報的資料模型、限制與 lease 續約
- 重要度：中
- 時間／階段：2026-10-22，票 11 實作
- 決策：(1) 三種回報（進度、checkpoint、受阻）共用一個 Domain 命令 `RunCommand::Report`，只有一個資料表 `agent_report`（`seq`、Workspace、Run、世代、種類、是否被接受、JSON 內容、UTC 毫秒）；被接受的回報同時追加 Run 事件（`run_progress_reported`、`run_checkpoint_saved`、`run_blocked_reported`），**checkpoint 的事件只記摘要，細節只存在 `agent_report`**。(2) 限制：摘要 1–500 字元、受阻原因 1–1000 字元、checkpoint 細節最多 16 KiB（位元組）；前後空白去除（細節保持原樣）；超過或空白回 400 `report_invalid`，不寫入任何東西。(3) 三種回報都**續約 lease**（等同 heartbeat），Starting 或 Running 才接受（Queued 拒絕）。AI-69 預告 `report_progress` 要呼叫 `Heartbeat`：這裡讓回報本身就是存活訊號，不另設獨立的 heartbeat 工具。(4) `RunView` 新增 `latest_progress`、`latest_checkpoint`（含 `details`）、`blocked`（沒有時為 null），所有回傳 Run 的端點與工作面板共用；面板顯示最新 Run 的三者，checkpoint 細節預設收合。
- 理由：回報種類少且結構相近，一張表加追加式事件最簡單，也符合 D-17 的「Domain 結果與事件同交易提交」。細節進事件會讓追加式事件表被大文字撐大且無法清理。16 KiB 足以放接手所需的下一步摘要，又不會讓每次 Run 清單都帶很大的內容。回報就是 Worker 還活著的證據，與 D-31 的 heartbeat 語意一致。
- 替代方案：三張表（各種類獨立查詢較方便，但三份幾乎相同的程式）；獨立的 `heartbeat` 工具（多一個工具、Worker 要記得呼叫）；checkpoint 細節放成果儲存（票 12 才有）。
- 影響：`GET …/runs`、`…/tasks` 會把每個 Run 的最新 checkpoint 細節一起回傳（最多 16 KiB × Run 數）；Run 很多以後需要把細節拆成單獨端點（屬分頁／清理的後續票）。`save_checkpoint` 目前只是被保存與顯示，接手者如何讀取（Context Builder 載入檢查點）屬票 15、28、39。進度回報頻率沒有限流。
- 審閱：未審

### AI-92：`get_task_contract` 回傳派工當下固定的契約版本
- 重要度：中
- 時間／階段：2026-10-22，票 11 實作
- 決策：`get_task_contract` 回傳 Run 的 `contract_version` 指到的那一版契約內容（Run 派工當下固定），加上 Task 識別與標題、Run 識別、世代與版本，不回傳授權內容或其他 Task 的資料。派工之後契約被改版（Task 非 `ready` 時允許），Worker 仍拿到原本那一版。
- 理由：D-07 的 Prompt 由 Task 契約產生，Run 啟動時就固定；Worker 中途拿到不同版本會讓「完成條件」在它不知情時改變（ADR-0011 要求 Worker 不能自行改寫完成標準）。契約變更如何通知進行中的 Worker 屬後續票（Gate 失效、訊息）。
- 替代方案：回傳最新契約（Worker 的目標會悄悄改變）；回傳最新版並附上「已有新版本」旗標（需要訊息機制，票 18）。
- 影響：本票只實作共同工具中的四個（`get_task_contract`、`report_progress`、`save_checkpoint`、`report_blocked`）；`register_artifact`、`request_decision`、`send_message`、`read_inbox`、`search_knowledge` 與 Controller 專屬工具由後續票加入。
- 審閱：未審

### AI-93：MCP shim 的結構與協定處理
- 重要度：低
- 時間／階段：2026-10-22，票 11 實作
- 決策：新增 crate `baton-mcp-shim`（lib＋bin），手寫逐行 JSON-RPC 2.0（`initialize`、`notifications/*`、`ping`、`tools/list`、`tools/call`），不依賴任何 MCP SDK；只依賴 `baton-domain`（取環境變數名稱與工具常數）、`tokio`、`reqwest`（關閉預設功能，不走 proxy）。協定版本：用戶端要求的版本在 `2024-11-05`、`2025-03-26`、`2025-06-18`、`2025-11-25` 內就回同一個，否則回 `2025-06-18`。Runtime 拒絕時回 MCP 工具錯誤（`isError: true`，文字是 `<代碼>：<說明>`），不是協定錯誤；連線失敗與逾時只回固定文字，不帶 reqwest 的錯誤內容。`serve`／`ShimConfig` 為 `pub`，harness 以 `tokio::io::duplex` 在程序內跑真正的 shim；另有真實 stdio 程序測試（缺少環境變數 exit code 2、輸出不含憑證）。`X-Baton-Client` 的標頭名稱與值在 shim 內各寫成一個常數（不依賴 `baton-api` 以免帶進整個 Axum 與 Store）。
- 理由：工具集小，手寫比引入 SDK 輕，也符合 spec「獨立輕量 Rust 程式」。shim 只負責轉送，所有判斷都在 Runtime，所以不需要知道 Workspace 或 Run。
- 替代方案：使用 `rmcp` 等 SDK（相依與編譯時間大增，協定細節仍要自己對照 Claude Code 的行為）。
- 影響：標頭值若在 `guard.rs` 改變，shim 內的常數要同步。**Validation Required（票 14）**：以真實 Claude Code 驗證協定協商、工具清單顯示與 `isError` 的呈現；shim 的實際輸出沒有 Claude Code 端的契約測試。
- 審閱：未審

### AI-94：子代理模型改為 Opus 5.5
- 重要度：中
- 時間／階段：2026-10-22，票 08、12、18、19、22 進行中
- 決策：從下一個新派出的 run 起，`claude -p` 改用 `--model claude-opus-5-5`，`--effort high` 不變；實作與整合範本的 Co-Authored-By 改為 Claude Opus 5.5。已在執行中的 R024、R025、R027、R028、R029 維持 Sonnet 5.5，不重派。
- 理由：使用者指定。
- 替代方案：維持 Sonnet 5.5。
- 影響：每個 run 的額度消耗預期上升，帳號較快接近 5h／7d 門檻；派工前檢查照舊。
- 審閱：認同 2026-10-22（使用者指定）

### AI-95：每個 Run 必屬一個帳號；派工要求交給 Scheduler，沒有名額就排隊而不是拒絕
- 重要度：高
- 時間／階段：2026-10-22，票 19 實作
- 決策：`Run` 新增 `profile_id`（派工當下選定、之後不變），沒有可派工的帳號 Profile 就不能建立 Run。`POST …/tasks/{id}/dispatch` 改由 Scheduler 處理：有帳號與名額 → 201 + Run（沿用票 10 的回應，多了 `profile_id`）；沒有帳號、名額已滿、額度達門檻、授權不足或相依未完成 → **202** + `{ queued: true, reasons }`，Task 維持 `ready` 並留在派工佇列。因此票 10 原本回 409 的 `dependencies_unfinished` 與「授權被撤銷」（`task_admission_denied` 中只含授權缺項者）改成 202 排隊；「不是等一等就好」的錯誤（Task 不是 ready、找不到、契約欄位或引用失效、已有寫入者）仍是原本的 409／404，且不寫入任何東西。
- 理由：票面要求「達到上限時新 Task 排隊並顯示原因」「阻擋原因區分額度、授權、相依與狀態不明」，原因必須在「被擋住的 Task」身上呈現；若授權與相依仍是 409，使用者看不到它們與容量並列的原因，Controller（票 17）「依賴未回答問題的 Task 不會被派出」也需要等待而非失敗。
- 替代方案：全部維持 409，只有容量排隊（授權、相依就沒有「等待」語意，與票 24 的撤權後行為不一致）；Run 沒有帳號也可建立、啟動時再選帳號（違反「Run 與帳號關聯正確」，且容量無法在派工交易內保證）。
- 影響：票 10 的兩個測試（撤銷授權、相依未完成）已隨之更新；測試與 Runtime 整合測試需要先有可派工的帳號（harness 新增 `add_account`）。`profile_id` 欄位在資料庫可為空（票 19 之前建立的 Run），Domain 以空字串表示，沒有外鍵（Profile 不刪除，只封存）。Controller 的 `dispatch_task` 工具（票 17）要處理 202。
- 審閱：未審

### AI-96：派工佇列只記錄「誰在排隊」；失敗後不自動重新排隊；優先序是要求時的整數
- 重要度：高
- 時間／階段：2026-10-22，票 19 實作
- 決策：新增 `dispatch_request(task_id 唯一, workspace_id, priority, requested_at)`。佇列只在使用者（或 Controller）要求派工時加入；Task 離開 `ready`（派出、取消、改契約退回 `pending`）時由 `save_task` 一併移出。Run 失敗、中斷或取消讓 Task 回到 `ready`，**不會**自動回到佇列（沿用 AI-66：沒有重試預算前不自動重派，否則失敗會造成無限重派迴圈）。「優先序」是派工要求本文的整數 `priority`（預設 0，越大越優先，同值依要求順序），沒有其他設定介面。等待原因**不落地**，每次由目前狀態重新判定。
- 理由：自動把每個 ready 的 Task 當作排隊會與 AI-66、票 36（重試預算）衝突；原因若存起來，授權或帳號狀態一變就過期。Task 目前沒有優先序欄位，調整優先序（使用者故事 25）屬 Controller 的票，這裡只提供 Scheduler 需要的排序鍵。
- 替代方案：ready 的 Task 全部自動排隊（失敗後無限重派）；在 Task 上加 `priority` 欄位（需要修改 Task 狀態機、契約與面板，超出本票範圍）。
- 影響：沒有 UI 設定優先序（API 支援）；對同一個排隊中的 Task 重複要求是冪等的，且可以更新其優先序。Run 失敗後使用者仍需手動再派一次。
- 審閱：未審

### AI-97：容量的兩組預設、保留交接槽的判斷、進行中的定義與選帳號規則
- 重要度：中
- 時間／階段：2026-10-22，票 19 實作
- 決策：兩組預設：「保留交接容量」（預設，即 spec D-14：每帳號 2、全域 6，可用帳號數 ≥ 3 時保留 1 個全域名額，一般派工最多 5 個）與「較高並行」（每帳號 3、全域 8、不保留）。「帳號數 ≥ 3」以**可用（使用中且核對無誤）的帳號數**計，不是 Profile 總數；帳號太少時實際保留數是 0。進行中的 Run = 尚未結束者（`queued`、`starting`、`running`，含已要求取消但程序還沒確認停止者，因為它仍持有寫入所有權與程序）。選帳號：在核對無誤、額度未達門檻、帳號名額未滿的帳號中挑進行中 Run 最少者，同數取較早建立的。切換預設存在 `scheduler_setting` 單列表，產生 `scheduler_settings_changed` 事件（相同模式不寫入）；切換後立即排程一次，已在執行的 Run 不受影響。
- 理由：spec 只說「可在較高並行／保留交接容量間切換預設組合」而沒有數字，「較高並行」的數字（3／8）是我定的：8 剛好是帳號數上限、每帳號 3 與 D-14 的 2 有明顯差異。以可用帳號數計保留槽，是因為被擋住的帳號無法接手交接，卻仍佔著保留名額會白白降低產能。最少者優先讓兩個帳號同時有空時各拿一個 Task（票面第一條）。
- 替代方案：以 Profile 總數計（簡單，但一個帳號被擋就浪費名額）；平手時輪流（需要額外狀態）；「較高並行」只拿掉保留槽而數字不變（切換前後差異太小）。
- 影響：這些數字都集中在 `CapacityMode::limits`，spec D-14 要求使用者回頭檢視；「保留交接容量」被命名為預設，但 spec 另提到的「保留較多交接容量」（story 47）目前沒有第三組。
- 審閱：未審

### AI-98：阻擋原因分六類（比票面多「名額已滿」與「帳號」）；額度只擋新派工、未觀測不擋
- 重要度：中
- 時間／階段：2026-10-22，票 19 實作
- 決策：`WaitCategory` = `capacity`（名額已滿）、`quota`、`authorization`、`dependency`、`state_unknown`、`account`。Profile 的阻擋原因對應：`profile_unverified`、`identity_unconfirmed`（還不知道）→ `state_unknown`；未登入、認證被覆蓋、設定目錄不符、身分不符、重複帳號、已封存與沒有任何帳號 → `account`。額度只實作 spec D-13 的第一道門檻：QuotaObserver 回報用量 ≥ 0.75 就不派新 Run 給該帳號（`quota_near_limit`）；Port 失敗、沒有樣本或用量未知的帳號**不阻擋**（未知不當作 0，也不因為還沒有觀測器而讓整個產品無法派工）。遲滯（<60% 才恢復）、≥85% 的 draining 與交接留給票 27、28。
- 理由：票面列了額度、授權、相依、狀態不明四類，但「名額已滿」（票面第二條）與「帳號未登入」是不同的處理方式，混進四類任何一類都會誤導；正式執行檔的 QuotaObserver 還是「尚未實作」，若把未知當阻擋，所有派工都會被擋。
- 替代方案：把容量與帳號問題硬塞進四類；未觀測就阻擋（保守，但在票 27 之前派不出任何 Run）。
- 影響：UI 需要六個類別標籤；票 27 加入遲滯時，只需讓 `quota_used` 的輸入包含「已進入停止派工狀態」的資訊或在 Scheduler 加入帳號狀態。`state_unknown` 也用於排隊期間發現「前一個 Run 還沒確認停止」（`execution_conflict`）這類不屬於其他類別的檢查失敗。
- 審閱：未審

### AI-99：交接槽（`SlotKind::Handoff`）只在 Domain 存在，本票沒有任何流程使用它
- 重要度：低
- 時間／階段：2026-10-22，票 19 實作
- 決策：Scheduler 的 `place` 接受 `SlotKind::Normal`（一般派工，扣掉保留槽）與 `SlotKind::Handoff`（可以用到全域上限）。本票所有派工路徑都是 `Normal`，交接槽「不被一般派工佔用」由 harness 情境測試證明（派滿一般名額後第 6 個排隊，總覽顯示保留數），「交接可以使用保留槽」只由 Domain 單元測試證明。
- 理由：交接流程（票 28）還不存在，沒有外部入口可以呈現交接派工；先把規則放在 Domain，票 28 直接呼叫即可。
- 替代方案：加一個測試專用的交接派工端點（把測試需求放進正式 API 面）。
- 影響：票 28 需要新增以 `SlotKind::Handoff` 派工的 Store 路徑，並補上「交接 successor 使用保留槽」的情境測試。
- 審閱：未審

### AI-100：排程在單一交易中完成，並在 Supervisor 的每次處理與派工 API 呼叫時執行
- 重要度：低
- 時間／階段：2026-10-22，票 19 實作
- 決策：`Store::schedule` 在一個寫入交易中載入全部 Profile、全域進行中的 Run 與容量設定，逐一判定並寫入，所以容量判定與 Run 建立不會被別的要求插隊。額度樣本是非同步 Port 呼叫，在交易外先取得再傳入（觀測不到的帳號不在結果中）。執行時機有三個：派工 API（同步，帶新要求）、容量預設切換（帶 `None`）與 `Supervisor::tick` 的最後一步（帶 `None`；派出新的 Run 時同一次 tick 再啟動它們）。Run 識別由呼叫端以閉包提供（Domain 不產生亂數）。佇列與等待原因不追加事件（事件只有 Run 的 `run_created`，其 payload 含 `profile_id`；容量切換有 `scheduler_settings_changed`）。
- 理由：「判定與寫入分開」會讓兩個同時到達的要求一起搶到最後一個名額；Supervisor 每秒處理一次，名額釋出後不需要另外的喚醒機制。排隊本身不是領域狀態變更（沒有版本、沒有轉換規則），所以不追加事件；時間線（票 41）需要時再加。
- 替代方案：判定在交易外、寫入在交易內再檢查一次（較複雜且仍要重判）；為排隊進入／離開追加事件（目前沒有消費者）。
- 影響：名額釋出到排隊 Task 啟動最長約 1 秒（背景迴圈間隔）；`harness.supervise()` 同一次呼叫就會把釋出的名額派出去。
- 審閱：未審

### AI-101：排程相關的 API 與 UI 形狀
- 重要度：低
- 時間／階段：2026-10-22，票 19 實作
- 決策：新增 `GET /api/v1/scheduling`（容量與使用量、每個使用中帳號的進行中 Run、已知用量、佇列與等待原因）與 `PUT /api/v1/scheduling/settings`（`{ "mode": "reserve_handoff" | "higher_concurrency" }`，回傳新的總覽）；Tauri 指令 `scheduling_overview`、`scheduling_set_mode`；`task_dispatch` 回傳 `DispatchOutcome`（Run，或帶 `queued: true` 的排隊結果）。UI：帳號頁每個帳號顯示「進行中 n／上限」與其進行中 Run；新增「並行容量」設定元件（導覽尚未存在，與 AI-37、AI-85 相同放在主區）；工作面板的 ready 卡片顯示「排隊中（第 N 位）」與依類別分組的原因，Run 狀態列顯示帳號名稱。
- 理由：帳號頁、設定頁與工作面板需要同一份總覽，單一端點避免各自重算；D-21 的導覽結構尚未存在。
- 替代方案：把進行中 Run 併進 `GET /profiles`（Profile 管理與排程混在一起，且總覽還要有佇列）。
- 影響：Controller 工具（票 17）與其他 UI 若要顯示等待原因，用同一個端點。
- 審閱：未審

### AI-102：資料表（migration 10）與既有資料
- 重要度：低
- 時間／階段：2026-10-22，票 19 實作
- 決策：migration 10 `scheduling`（接在票 11 的 `agent_credential_scope`、`agent_report` 之後）：`run.profile_id`（可為空、無外鍵，並建立索引）、`dispatch_request`（`task_id` 唯一）、`scheduler_setting`（單列，`CHECK (id = 1)`）。`run.profile_id` 不設外鍵，因為新增欄位時 SQLite 的外鍵欄位必須預設為 NULL，而 Domain 以選帳號的結果保證關聯正確；Profile 只封存不刪除，所以不會有懸空參照。
- 理由：尚無發布版本（AI-86 同樣前提），不需要回填舊 Run 的帳號。
- 替代方案：重建 `run` 表加外鍵（對尚無資料的開發版本沒有價值，且風險高）。
- 影響：本票原本是第 8 個 migration，整合 rebuild/v1（票 11 已有第 8、9 個）時順延為第 10 個；AI 編號同樣由 AI-87～AI-94 順延為 AI-95～AI-102。
- 審閱：未審

### AI-103：`register_artifact` 的內容以參數內嵌傳入，不接受 Worker 指定的路徑
- 重要度：高
- 時間／階段：2026-10-22，票 12 實作
- 決策：Worker 呼叫 `register_artifact` 時，內容直接放在參數裡：`content`（UTF-8 文字）或 `content_base64`（二進位），二選一；單一成果最多 10 MiB（`ARTIFACT_MAX_BYTES`）。Runtime 不讀取請求中的任何檔案路徑。
- 理由：Worker 是不完全可信的程序，若登記時可指定路徑，Runtime 會替它讀取該路徑的檔案並存進成果儲存，再經由成果下載交出去，等於讓 Worker 藉 Runtime 的身分讀走工作目錄以外的檔案（例如其他帳號的登入資料目錄）。內嵌內容讓「Worker 能提供的資料」只限於它自己放進參數的位元組。
- 替代方案：接受 Workspace／worktree 內的相對路徑，由 Runtime 讀取。可以支援大型檔案，但必須先有 worktree 的位置規則與路徑逃逸（符號連結、junction、`..`）防護，屬於票 20 之後再評估。
- 影響：大型成果（超過 10 MiB，例如影片、資料集）目前無法登記；JSON 內嵌會使 MCP 本文約為內容的 1.4 倍（Base64），Runtime 的登記路由因此把本文上限設為 32 MiB。若日後要支援大檔，需新增「由 Runtime 讀取受限路徑」的第二條路徑並另行審閱。**Validation Required（票 14）**：Claude Code 對單一 MCP 工具呼叫的參數大小是否有自己的上限，需要以真實 Worker 驗證。
- 審閱：未審

### AI-104：成果檔案的儲存位置、寫入順序與「未登記檔案」不自動刪除
- 重要度：中
- 時間／階段：2026-10-22，票 12 實作
- 決策：成果存放在 `<資料目錄>\artifacts\<workspace_id>\blob_<隨機>`，每個版本一個獨立檔案、檔名與成果名稱無關。寫入順序是：暫存檔 `<檔名>.partial` → fsync → 原子改名 → 在單一交易提交 manifest 與事件。中途中斷只會留下「未登記檔案」：`.partial` 暫存檔（`staging`，內容不完整）或已改名但 manifest 未提交的檔案（`unregistered`，內容完整但不算成果）。`GET …/artifacts` 的 `unregistered_files` 列出它們，恢復時可辨識；Runtime **不自動刪除**，也不在啟動時搬移。`WorkspaceFs` Port 新增 `read_artifact`、`list_artifact_files`，`write_artifact_atomic` 補上正式實作；fake 在三個點（`MidWrite`、`BeforeRename`、`AfterRename`）可模擬崩潰。
- 理由：「已登記」只由 manifest 決定，而 manifest 一定在檔案完整落地之後才提交，所以不需要「恢復時修復」就能保證已登記的成果完整。spec 非目標明列「自動刪除正式 Artifact」，未登記檔案雖不是成果，但是否刪除涉及使用者資料，先保守地只顯示。
- 替代方案：啟動時自動刪除 `.partial`（暫存檔一定無用，刪除安全）；先只顯示是因為清理屬於票 21／40 的範圍，且不刪除不影響正確性。
- 影響：崩潰多次後儲存區會累積殘留檔案，直到後續票加入清理或備份流程；列表每次都會掃描 Workspace 的成果目錄（成果數量大時需要改成索引）。
- 審閱：未審

### AI-105：成果與版本的識別：同 Workspace、同 Task、同名是同一個成果
- 重要度：中
- 時間／階段：2026-10-22，票 12 實作
- 決策：成果以（Workspace、Task、名稱）唯一識別；再次登記同名成果只追加新版本（版本號從 1 起連續遞增），不同名稱是另一個成果。Task 與產生者（Run、世代）一律取自憑證所屬的 Run，請求不能指定。`artifact`、`artifact_version` 是追加式資料表（觸發器禁止更新與刪除，`storage_path` 唯一），每個版本追加事件 `artifact_registered`（`aggregate_type = 'artifact'`，聚合版本 = 成果版本號）。同一個 Run 重送相同內容不去重，會產生新版本。
- 理由：票面要求「同一成果新版本不覆寫舊版本」，需要一個穩定的「同一成果」定義；Worker 重新產生報告時自然會沿用同一個檔名。以 Task 為界避免不同 Task 同名互相混成同一個成果。
- 替代方案：由 Worker 在登記時提供成果識別（容易被亂用或撞名）；以內容雜湊去重（Worker 重送會被悄悄合併，但無法表達「再次確認同一內容」，且增加判斷）。
- 影響：不同 Task 之間沒有版本關係；Task 重派後的新 Run 仍歸入同一個成果（同 Task 同名）。票 13 的驗證 Gate 若需要「某個版本被接受」的狀態，要另建紀錄，不可改寫版本表。
- 審閱：未審

### AI-106：預覽與下載重新核對雜湊；列表只比對大小；桌面殼以 base64 傳遞下載內容
- 重要度：中
- 時間／階段：2026-10-22，票 12 實作
- 決策：預覽與下載每次都重新讀檔、計算 SHA-256 並比對大小，與 manifest 不符回 409 `artifact_integrity_failed`，檔案不存在回 409 `artifact_file_missing`，不交出內容。列表的 `file_status`（`ok`／`missing`／`size_mismatch`）只比對存在與大小，避免列表為每個版本讀整個檔案。管理 API 只接受桌面憑證（既有 `guard` 規則），Agent 憑證不能下載；成果一律以 Workspace 為界，跨 Workspace 或不存在一律回 404 `artifact_not_found`。Tauri 殼的 `artifact_download` 指令把內容以 base64 放在 JSON 回傳值裡交給 Web UI（Web UI 再核對 `X-Baton-Sha256` 與 manifest 雜湊）。
- 理由：票面要求「manifest 雜湊與檔案一致」且「下載同樣驗證身分」；成果檔案放在使用者可編輯的資料目錄，事後被改動或截斷是可能的，交出不可信的內容比拒絕更糟。base64 是 Tauri JSON 指令最簡單的作法，上限 10 MiB 時約 14 MiB，可接受。
- 替代方案：列表也計算雜湊（成本高）；以 `tauri::ipc::Response` 回傳原始位元組（省 33% 傳輸，但前端與殼都要另一套處理）；預覽時不核對（較快，但預覽可能顯示被竄改的內容）。
- 影響：大成果的預覽與下載每次都要讀整個檔案。使用者要「忽略完整性失敗仍想取回檔案」沒有入口，需要時直接從資料目錄取得。
- 審閱：未審

### AI-107：成果登記的限制值與未涵蓋的行為
- 重要度：低
- 時間／階段：2026-10-22，票 12 實作
- 決策：名稱必須是檔名（不含路徑分隔字元、控制字元與 Windows 不允許的 `:*?"<>|`，不可是 `.`／`..`，最多 200 字元）；描述最多 1000 字元；預覽最多 64 KiB 並在字元邊界截斷，含 NUL 或不是有效 UTF-8 者視為 `binary`。登記**不改變 Task 狀態**（接受成果屬票 13 的驗證 Gate）、**不續約 lease**（只有三個回報工具與 Heartbeat 續約）；過期世代的登記回 409 `run_report_stale`，不寫入任何檔案、也不像三個回報工具那樣留稽核紀錄。
- 理由：數值是預設值，改起來便宜；不續約 lease 與不留稽核都是為了讓本票只做票面要求的事，避免提早決定後續票的語意。
- 替代方案：登記同時視為存活訊號並續約 lease；過期世代的登記也留稽核（需要保存被拒絕的內容，成果可達 10 MiB，不適合放進追加式資料表）。
- 影響：Worker 若長時間只登記成果而不回報進度，lease 仍會照原本的規則過期並被核對。
- 審閱：未審

### AI-108：票 12 整合 rebuild/v1（票 19）：migration 與 AI 編號順延、衝突合併方式
- 重要度：低
- 時間／階段：2026-10-22，票 12 整合（R036-12-integrate）
- 決策：票 12 的 migration `artifact` 原本是第 10 個，與票 19 的 `scheduling`（migration 10）重號，順延為第 11 個，排在 `scheduling` 之後；兩者互不參照（`artifact_version.run_id` 只參照 `run.id`），所以先後順序不影響 schema。票 12 的 AI 編號由 AI-95～AI-99 順延為 AI-103～AI-107，內容不變；程式碼與文件沒有引用這些編號，不需同步。其餘衝突（API 路由、Tauri 指令清單、`AppClient` 型別、開發指南的 crate 表與章節）一律兩邊都保留：路由與指令並列，`AppClient` 同時包含 `SchedulingClient` 與 `ArtifactClient`，開發指南「多帳號並行排程與容量」在前、「成果儲存與成果瀏覽」在後。票 19 起派工必須有可派工帳號、`dispatch_task` 改回傳 `DispatchOutcome`，所以本票的 `crates/baton-harness/tests/artifacts.rs` 與 `crates/baton-client/tests/artifacts_client.rs` 在共用設定中加上 `harness.add_account`（與票 19 調整 `agent_api.rs` 的方式相同），並把派工結果拆成 `Started`；測試的斷言不變。
- 理由：整合分支已有的 migration 與 AI 編號不可變動；已套用的資料庫以 migration 序號判斷，只能把後合併的一方往後排。
- 替代方案：把兩個 migration 合併成一個（會改動整合分支已有的 migration 10，已套用的開發資料庫會漏掉其中一半）。
- 影響：以票 12 分支（合併前）建立過的開發資料庫，其 `schema_migrations` 第 10 筆是 `artifact`，與整合後的 `scheduling` 不一致，需刪除重建；尚無發布版本，不提供轉換。
- 審閱：未審

### AI-109：已驗證通過的票，整合後不再重驗
- 重要度：中
- 時間／階段：2026-10-22，票 08、12、18、22 合併
- 決策：票的驗證 run 判定 PASS 之後，若因其他票先合併而需要整合 run，整合 run 跑完全部測試且通過，就直接合併，不再派重新驗證。驗證 FAIL 後的修正 run 仍要重新驗證。
- 理由：08、12、18、22 都把 migration 排在第 11 個、AI 編號都從 AI-103 起，每合併一張，其餘都要再整合；若每次整合後都重驗，驗證 run 的數量會隨平行票數成倍增加。整合 run 不改票的行為，且被要求不可刪除或弱化任何測試。
- 替代方案：照 AI-75 每次整合後重驗（較保險、額度消耗大）；或把會動到 Store／API 的票改成循序派工（降低並行度）。
- 影響：整合過程若引入行為缺陷（例如 R037 發現的「排程流程不再附上未讀摘要」），只能靠整合 run 自己的測試與判斷抓出；最終的 code-review 會再檢查一次整合分支。
- 審閱：未審

### AI-110：匯入的來源白名單與位置
- 重要度：高
- 時間／階段：2026-10-22，票 08 實作
- 決策：「使用者平常的 Claude Code 設定」只讀三個檔案：`settings.json`（偏好、hooks、`enabledPlugins`、`extraKnownMarketplaces`）、`CLAUDE.md`（全域指示）、`.claude.json` 中**只有** `mcpServers`（使用者層級 MCP）。位置：設定了 `CLAUDE_CONFIG_DIR` 時為該目錄（`.claude.json` 在其中），否則是 `%USERPROFILE%\.claude` 與 `%USERPROFILE%\.claude.json`。`.credentials.json` 與 `.claude.json` 的其他欄位（登入帳號、使用者識別、各專案資料）永遠不讀。讀檔經新的 `UserSettingsSource` Port，它沒有任何寫入能力。`settings.local.json`、專案層設定、skills／agents／commands 目錄不在本票範圍。
- 理由：票面五類（偏好、指示、hooks、MCP、plugins）正好對應這三個檔案；白名單讓「不複製登入資料」由結構保證（根本不讀），而不是靠事後清洗。
- 替代方案：整個 `~/.claude` 目錄複製再清洗（會碰到登入檔與 Session 資料）；讓使用者自選來源目錄（多一個輸入欄位、容易指到 Profile 目錄）。
- 影響：使用者若把 MCP 設定放在專案層 `.mcp.json` 或把 Skills 當作「共同基準」，不會被匯入；要支援需新增來源。`CLAUDE_CONFIG_DIR` 若被 Runtime 的啟動環境設成某個 Profile 目錄，來源就會是那個 Profile 的目錄（正式使用時使用者不會這樣啟動）。
- 審閱：未審

### AI-111：秘密處理的規則與限制
- 重要度：高
- 時間／階段：2026-10-22，票 08 實作
- 決策：三層。(1) 登入相關一律排除：`apiKeyHelper`、`awsAuthRefresh`、`awsCredentialExport`、`forceLoginMethod`、`forceLoginOrgUUID`、`otelHeadersHelper`、`oauthAccount`、`primaryApiKey`、`customApiKeyResponses`，以及 `settings.json` 的 `env` 中 `ANTHROPIC_*`、`CLAUDE_CODE_OAUTH*`、`CLAUDE_CODE_USE_*`、`AWS_*`、`GOOGLE_APPLICATION_CREDENTIALS*`（這些會覆蓋 Profile 的原生登入，與 AI-78 的認證來源檢查衝突）。(2) 欄位名稱像秘密（含 token、secret、password、apikey、authorization、credential、cookie 等；`tokens` 複數視為用量上限不算）且值是非空字串時，只丟該欄位、保留其餘項目。(3) 字串內容像秘密（`sk-ant-`／`sk-`／`ghp_`／`github_pat_`／`xox*-`／AKIA／JWT／`Bearer …`／`名稱=值`／含密碼的網址／PEM 私鑰），整個項目（一個 hook 事件、一個 MCP 伺服器、一個偏好、整份 `CLAUDE.md`）都不匯入。`${NAME}` 形式的環境變數參照不是秘密，保留。排除紀錄只有「來源、項目路徑、原因」，絕不含值。這是啟發式規則，抓不到沒有可辨識形狀的秘密（例如 MCP `args` 裡隨意命名的密碼）。
- 理由：驗收要求「匯入結果不含任何登入秘密」；欄位層級的丟棄讓常見的 `env: { GITHUB_TOKEN }` 不致整個 MCP 伺服器都被拒；內容層級的整項排除避免「改寫使用者的指示或命令」。`CLAUDE.md` 整份排除而不是逐行塗黑，是因為塗黑會悄悄改變指示的意思。
- 替代方案：把秘密換成佔位符後保留（Run 啟動時伺服器會壞，且佔位符本身會被當成設定）；只依名稱判斷（漏掉 `--key=sk-…` 這類）；只依內容判斷（漏掉長得不像金鑰的密碼）。
- 影響：被排除的 MCP 伺服器在 Run 裡不存在，使用者要在預覽的排除清單看到原因，自行改用 `${VAR}` 參照或在其他地方提供。偵測規則改版（新增金鑰格式）只需改 `shared_settings/secrets.rs`；既有版本不會被重新掃描。測試以「在 API 回應、快照目錄、資料庫檔（含 WAL）與日誌中掃描已知秘密」驗證，但掃描只涵蓋測試放進去的那些樣式。
- 審閱：未審

### AI-112：預覽、採用與版本的語意
- 重要度：中
- 時間／階段：2026-10-22，票 08 實作
- 決策：預覽不保存任何東西（沒有「待採用」暫存狀態）。採用必須帶預覽回傳的 `candidate_hash`，Runtime 重新讀取來源後若雜湊不同回 409 `settings_source_changed`，所以使用者採用的一定是看過的內容。**每次採用都建立新版本**，即使內容與目前版本相同（版本號連續遞增、雜湊相同）；預覽以 `changed` 與空差異告知，UI 在差異為空時停用「採用」。版本以追加式資料表保存（觸發器禁止更新與刪除），建立時同交易追加 `shared_settings_adopted` 事件（聚合版本 = 版本號）。內容雜湊 = 排序後標準 JSON 的 SHA-256，與來源檔的排版、鍵順序無關；排除清單與來源指紋不參與雜湊。
- 理由：票面寫「每次採用建立新版本」，照字面做最不會與驗證者的測試落差；以雜湊做樂觀比對比存暫存候選簡單，也不會有過期的暫存。
- 替代方案：內容相同時冪等回傳原版本（較整潔，但與票面字面不同）；預覽時存候選並以識別採用（需要清理過期候選）。
- 影響：連按兩次「採用」會產生兩個內容相同的版本（無害）。沒有任何版本且來源全空時 `changed` 為 true 但差異為空，UI 因此無法採用空內容。
- 審閱：未審

### AI-113：Run 設定快照的形狀與 Run 綁定
- 重要度：高
- 時間／階段：2026-10-22，票 08 實作
- 決策：快照目錄是 `<資料目錄>\run-settings\<run_id>\`，內容只含有資料的類別：`settings.json`（偏好＋hooks＋`enabledPlugins`＋`extraKnownMarketplaces`，Claude Code 設定格式）、`mcp.json`（`{"mcpServers": …}`）、`CLAUDE.md`，外加 `manifest.json`（版本、內容雜湊、各檔 SHA-256）。渲染完全由版本內容決定（鍵排序、固定縮排）；`bundle_from_snapshot_files` 可從目錄還原內容並重算雜湊，與版本雜湊一致。Run 與版本的綁定存在追加式表 `run_settings_snapshot`：第一次要求時綁定（預設最新版本，也可指定），之後再要求回傳同一份（200），要求別的版本回 409 `settings_snapshot_pinned`，所以後來採用的新版本不會影響已准入的 Run。端點 `POST /api/v1/workspaces/{id}/runs/{run_id}/settings-snapshot`（桌面憑證），內部能力是 `materialize_run_settings`。本票**沒有**讓 Supervisor 在派工啟動時自動呼叫它，也沒有把快照檔以設定參數傳給 Claude Code：正式的 AgentAdapter 還是「尚未實作」，且「Claude Code 能否完全擋下使用者層設定合併與熱重載」是 spec 的 Validation Required。
- 理由：票面要求「提供」取得版本快照並寫成 Run 專屬唯讀目錄的能力；接到派工流程與 CLI 參數屬於 Adapter／Supervisor 的票。綁定放資料庫（而不是只靠目錄存在）才能在重啟後仍回答「這個 Run 用哪一版」。
- 替代方案：在派工交易裡就綁定版本（改動票 10 的 Dispatch 命令與測試，範圍超出本票）；目錄名帶版本號（同一 Run 若競爭綁定會留下孤兒目錄）。
- 影響：後續的 Adapter 票要在啟動 Run 前呼叫它（或把它匯出給 Supervisor：目前是 `baton-api` 內的 `pub(crate)`），並把 `settings.json`、`mcp.json` 與 `CLAUDE.md` 以 `--settings`、`--mcp-config` 與附加指示的方式傳入；是否需要同時設定 `--setting-sources` 排除使用者層需實測。Run 未綁定版本就派工目前不會被阻擋。
- 審閱：未審

### AI-114：快照目錄的「唯讀」只靠檔案唯讀屬性，不防刪除
- 重要度：中
- 時間／階段：2026-10-22，票 08 實作
- 決策：快照檔寫入後設為唯讀屬性（目錄也設），先寫同層暫存目錄再改名，目錄已存在時整個取代（取代前先解除唯讀）。實測（本機 Windows 11）：唯讀屬性能阻止開啟寫入與覆寫，但**不阻止刪除**——Rust 標準函式庫與 Node 的 `unlink` 都會忽略唯讀屬性；目錄的唯讀屬性也不阻止新增檔案。不另外加 ACL。情境測試只斷言「不能寫入、不能改寫」。`manifest.json` 記錄各檔雜湊，可偵測竄改，但目前沒有流程自動核對或修復。
- 理由：同一 Windows 使用者本來就能取消任何唯讀屬性，ACL 也擋不了同一使用者；票面的目的是「避免熱重載改變已准入 Run」，也就是防止 Claude Code 或使用者無意間改檔，而不是抵禦惡意。
- 替代方案：以 ACL 拒絕刪除與寫入（Windows 專屬、清理時要先還原、測試複雜）；每次回傳快照時核對並修復目錄（需要新增 WorkspaceFs 讀取能力）。
- 影響：快照被刪除或竄改時 Run 不會被通知；後續若要保證，Adapter 啟動前應核對 `manifest.json` 並在不符時重新產生。已寫入 `docs/development.md` 的 Validation Required。
- 審閱：未審

### AI-115：新增 UserSettingsSource Port 與 WorkspaceFs 的寫入唯讀目錄
- 重要度：中
- 時間／階段：2026-10-22，票 08 實作
- 決策：`Ports` 多一個欄位 `user_settings: Arc<dyn UserSettingsSource>`（`read() -> SourceFiles`，不存在的檔案以 `bytes: None` 回報）；`WorkspaceFs` 多一個方法 `write_readonly_files(dir, files)`。唯讀讀檔與寫唯讀目錄的實作（`DirUserSettingsSource`、`write_readonly_tree`）放在 `baton-ports`，正式版與 harness 的 fake 共用，因為這是對真實檔案系統的操作。harness 的 `FakeUserSettings` 是一個暫存「家目錄」（`.claude\`、`.claude.json`），讀取走真實讀檔，所以測試能直接比對使用者原始檔案的位元組，也能放入 `.credentials.json` 證明它不被讀。正式 `LocalWorkspaceFs` 順便實作了 `write_readonly_files`；`write_artifact_atomic` 仍是「尚未實作」（不在本票範圍）。
- 理由：「讀使用者設定」與「寫快照目錄」都是外部副作用，依 D-04 必須經 Port；與 AI-79 相同的作法。
- 替代方案：API 直接讀寫檔案（harness 無法替換位置，測試會碰到真實的家目錄）。
- 影響：之後建立 `Ports` 的地方要補上 `user_settings`（已更新 production、harness 與 Runtime 整合測試）。
- 審閱：未審

### AI-116：共同設定頁面的位置與錯誤代碼辨識
- 重要度：低
- 時間／階段：2026-10-22，票 08 實作
- 決策：新增 `SharedSettingsPage`，掛在 App 主區帳號頁之後（導覽尚未存在，與 AI-85 相同）；沒有新增 CSS。Tauri 指令 `shared_settings_status`、`shared_settings_preview`、`shared_settings_adopt`、`shared_settings_version`；HTTP 版用戶端對應同一組端點。既有前端沒有辨識穩定錯誤代碼的機制，新增 `errorCode(error)`：HTTP 版把代碼掛在 `Error.code`，Tauri 版從 `ClientError` 訊息格式中取出；採用被 `settings_source_changed` 拒絕時清掉過期的預覽並要求重新預覽。
- 理由：需要在 UI 區分「來源已改變」與其他錯誤。
- 替代方案：把 `error` 代碼併進訊息文字再比對字串。
- 影響：`ClientError` 的訊息格式若改變，前端的辨識正規表示式要同步改；沒有視覺樣式，之後做導覽時一併處理。
- 審閱：未審

### AI-117：票 08 整合 rebuild/v1：migration 順序與文件
- 重要度：低
- 時間／階段：2026-10-22，票 08 實作
- 決策：整合分支此時已有票 11 的兩個 migration（第 8、9 版：`agent_credential_scope`、`agent_report`），本票的 `shared_settings_version`（含 `run_settings_snapshot`）接在其後成為第 10 版；兩邊的 migration、API 路由、`Ports`／`ApiState` 欄位、`docs/development.md` 的表格列都完整保留。本票條目編號接在整合分支最大的 AI-93 之後（原為 AI-95～AI-102；第二次整合時順延為 AI-103～AI-110，見 AI-119；第三次整合時再順延為 AI-110～AI-117，見 AI-120）。
- 理由：已進入整合分支的 migration 不可改動，順延尚未合併的本票。
- 替代方案：無。
- 影響：若其他平行票先合併並也新增 migration 或 AI 編號，需由整合 run 再順延一次（與 AI-86 同）。
- 審閱：未審

### AI-118：名稱本身像秘密時，排除紀錄以「<已遮蔽>」代替；偵測補上 `名稱: 值` 標頭形式
- 重要度：高
- 時間／階段：2026-10-22，票 08 實作（R031 修正 R030 驗證）
- 決策：(1) 偏好鍵、hook 事件、MCP 伺服器、plugin／市集名稱，以及 `env` 的登入變數名稱，若名稱本身含有像秘密的內容，排除紀錄的項目路徑改寫成 `<已遮蔽>`（例如 `mcpServers.<已遮蔽>`；頂層偏好鍵只寫 `<已遮蔽>`），原名稱不進入排除清單、資料庫、預覽 API 與 UI。項目內部的子欄位名稱若像秘密，整個項目以上層路徑排除（原本就是如此）。(2) 字串內容偵測新增標頭形式：`名稱: 值` 或 `名稱:值`，名稱符合秘密欄位規則（如 `x-api-key`、`Authorization`），且值至少 16 字元、只含 token 字元、英數混合時，視為秘密（例如 hook 指令 `curl -H 'x-api-key: abcdefghijklmnop1234567890'`）。`${VAR}` 參照與一般文字不受影響。
- 理由：R030 驗證實測到 `{"ghp_…":true}` 與以金鑰為名的 MCP 伺服器，金鑰會原樣寫進 `Exclusion.item`，違反「匯入結果不含任何登入秘密」與 AI-111「排除紀錄絕不含值」。標頭形式是驗證者提出的實測漏網案例，是 hook 與 MCP 設定中最常見的金鑰寫法之一。
- 替代方案：以雜湊或部分字元代替名稱（仍可能洩漏片段或被比對）；名稱像秘密時完全不留紀錄（使用者看不到有東西被排除）；標頭只看名稱不看值（`password: 請向管理員索取` 這類文字指示會被誤判，連帶整份 `CLAUDE.md` 被排除）。
- 影響：多個被遮蔽的同類項目在排除清單裡無法區分，使用者要回原始檔自行找出。標頭規則要求值英數混合、至少 16 字元，純字母或較短的密碼仍抓不到（AI-111 的限制不變）；名稱像 `tokenizer` 這類被當成秘密欄位的誤判，可能讓含 `tokenizer: model_v2_0123456789` 的指示被排除。
- 審閱：未審

### AI-119：票 08 第二次整合 rebuild/v1：migration 第 11 版、AI 編號順延、情境測試補帳號
- 重要度：低
- 時間／階段：2026-10-22，票 08 實作（R031 修正時整合）
- 決策：整合分支已先合併票 19（多帳號排程），其 migration `scheduling` 是第 10 版、決策條目用了 AI-95～AI-102。本票的 `shared_settings_version` 順延為第 11 版；本票條目由 AI-95～AI-103 順延為 AI-103～AI-111，並移到票 19 條目之後依編號排列，程式碼與文件中的引用（AI-99 → AI-107）同步更新。票 19 讓派工必須有可用帳號、並把 `RuntimeClient::dispatch_task` 改回傳 `DispatchOutcome`，所以本票的情境測試在派工前為每個 Run 建立一個帳號（避免同一測試的第 3 個 Run 碰到每帳號上限而排隊），client 測試改為比對 `DispatchOutcome::Started`。`ApiState`／`Ports` 欄位、API 路由、前端 `AppClient`、Tauri 命令與 `docs/development.md` 兩邊的內容都完整保留。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票（與 AI-86、AI-117 同）。
- 替代方案：讓情境測試共用一個帳號並改用較高並行預設（會把排程設定混進共同設定的測試）。
- 影響：已用本票分支建過的開發資料庫，`shared_settings_version` 記在第 10 版，與整合後的編號不一致，需要刪除重建。
- 審閱：未審

### AI-120：票 08 第三次整合 rebuild/v1：migration 第 12 版、AI 編號再順延
- 重要度：低
- 時間／階段：2026-10-22，票 08 整合（R044-08-integrate）
- 決策：整合分支已先合併票 12（成果儲存），其 migration `artifact` 是第 11 版、決策條目用了 AI-103～AI-109。本票的 `shared_settings_version`（含 `run_settings_snapshot`）順延為第 12 版，排在 `artifact` 之後；兩者互不參照（都只參照 `run.id`），所以兩邊的 schema 都會依序套用。本票條目由 AI-103～AI-112 順延為 AI-110～AI-119，移到票 12 條目之後依編號排列；條目內的交互引用（AI-104 → AI-111、AI-110 → AI-117、AI-112 → AI-119）與程式碼、文件中的引用（AI-107 → AI-114：`docs/development.md`、`crates/baton-harness/tests/shared_settings.rs`）同步更新，記錄歷史編號的敘述保留原文。原 AI-112 缺少審閱行，順延時補上 `- 審閱：未審`。其餘衝突一律兩邊都保留：`WorkspaceFs` 同時有成果的 `read_artifact`／`list_artifact_files` 與本票的 `write_readonly_files`（正式版各自實作），`baton-ports` 同時匯出 `write_file_atomic` 等成果工具與 `write_readonly_tree`、`UserSettingsSource`；API 路由、`RuntimeClient` 方法、Tauri 指令與匯入、前端 `AppClient`（`ArtifactClient & SharedSettingsClient`）與 `errorCode`、前端測試、開發指南的 crate 表四列合併為同時描述兩張票的內容。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票（與 AI-86、AI-108、AI-117、AI-119 同）。
- 替代方案：把兩個 migration 合併成一個（會改動整合分支已有的第 11 版，已套用的開發資料庫會漏掉其中一半）。
- 影響：已用本票分支建過的開發資料庫，`shared_settings_version` 記在第 11 版，與整合後的編號不一致，需要刪除重建（與 AI-119 同）。
- 審閱：未審

### AI-121：QuotaObserver Port 改為「外部觀測器」；D-12 的兩個來源由 Runtime 從 Adapter 事件累積
- 重要度：高
- 時間／階段：2026-10-22，票 27 實作
- 決策：spec 把「已觀測」與「推估」都寫成 QuotaObserver 的實作，但 harness 以 fake QuotaObserver 取代整個 Port 時就測不到它們。因此兩個來源改由 Runtime 自己累積：Adapter 送出 `AdapterEvent::RateLimited { window, status(allowed／warning／exhausted), used, reset_at_millis, raw }`（取代原本只有 `reset_at_millis` 的形狀）與新增的 `AdapterEvent::UsageReported { tokens, raw }`，Supervisor 收到後記為該 Run 帳號（`run.profile_id`）的觀測；沒有帳號的舊 Run 略過。QuotaObserver Port 保留為 spec 所說的「第三個觀測器」擴充點（例如 CLI 直接的額度查詢），每個帳號每 5 分鐘輪詢一次；正式版仍回 `NotImplemented`（不留紀錄）。`QuotaSource`、`Confidence` 移到 `baton_domain::quota`（ports 重新匯出，序列化改為 snake_case），`AdapterEvent` 因帶 `f64` 不再 `Eq`，`baton_ports::observe_quota` 移除（改用 `baton_api::quota`）。
- 理由：主要 seam 只替換 Port（D-04）；Adapter 事件才是真正的外部邊界，用 scripted fake AgentAdapter 驅動兩個來源，情境測試才會經過真實的判斷與持久化。保留 Port 讓既有的 fake 樣本（票 19 的測試）與未來的直接查詢都能沿用。
- 替代方案：照 spec 把兩個來源寫成 QuotaObserver 實作、由 Runtime 組合（harness 換掉 Port 後測不到它們，或 harness 必須改用真實實作而失去 fake 的意義）；移除 Port（違反 D-04 的 Port 清單）。
- 影響：**票 14** 的真實 Adapter 必須從 stream-json 解析並送出這兩種事件（格式為 Validation Required），契約測試要涵蓋；`tokens` 計入哪些 token 類別（建議 input + output + cache 建立）由票 14 決定。
- 審閱：未審

### AI-122：「較保守者」與已觀測訊號的判斷規則
- 重要度：中
- 時間／階段：2026-10-22，票 27 實作
- 決策：每個視窗取已觀測與推估中用量較高者（同值偏好已觀測），帳號再取兩個視窗與外部觀測器中最高者，這個值就是排程用的用量（`quota_used`，≥75% 停止派工沿用票 19）。`exhausted` 視為 100%；`warning` 沒有數字時視為 75%（停止派工門檻，UI 標示「CLI 警告，至少此值」）；`allowed` 沒有數字是未知。沒有說明視窗的訊號歸到五小時視窗。訊號在 `reset_at` 之後失效、沒有重置時間時維持一個視窗長度（5 小時或 7 日），失效後回到未知（不是 0）；同一視窗以最新一筆訊號為準。
- 理由：票面要求並存時取較保守者；CLI 的警告代表接近上限，沒有數字時至少不該再派新工作。沒有視窗資訊的訊號歸到較短的視窗，避免一則訊號就把帳號鎖住七天。
- 替代方案：警告沒有數字時視為未知（可能繼續派工到撞牆）；無視窗訊號同時套用兩個視窗並維持七日（過度保守）。
- 影響：遲滯（<60% 才恢復）、≥85% 的 draining 與「收到警告就交接」屬票 28，本票只提供判斷值與訊號狀態。若真實 CLI 的訊號帶有其他狀態，票 14 需對應到這三種。
- 審閱：未審

### AI-123：推估模型：token 為單位、使用者設定方案上限；沒有依據時是未知
- 重要度：中
- 時間／階段：2026-10-22，票 27 實作
- 決策：推估 = 滾動視窗內本機 Run 回報的 token 累計 ÷ 該視窗的方案上限（token）。方案上限每個 Profile 兩個（五小時、七日），可留空；沒有上限，或視窗內沒有任何本機用量時，用量是未知（記下 `tokens` 與 `limit_tokens`，但 `used` 為 null、可信度 unknown）。推估的重置時間是視窗內最早一筆用量滑出視窗的時間。比例可以超過 100%。
- 理由：Claude 的方案上限沒有公開的 token 數，只能讓使用者填；「本機沒有用量」不代表帳號沒有被使用（其他裝置看不到），顯示 0% 會違反「觀測不到時顯示未知而非 0」。
- 替代方案：以費用（USD）為單位（結果訊息有 `total_cost_usd`，但訂閱方案的上限也不是以費用公告）；本機沒有用量時推估為 0%（較直觀，但與驗收條件衝突）。
- 影響：使用者需要自行估計方案上限才能得到推估比例；沒有填時只有已觀測來源能擋派工。
- 審閱：未審

### AI-124：「觀測完整」的定義
- 重要度：中
- 時間／階段：2026-10-22，票 27 實作
- 決策：只有兩個視窗都有來自 CLI、仍有效、帶數字（或 `exhausted`）的已觀測訊號，或外部觀測器有「已觀測」且帶數字的樣本時，才算 `complete`；否則 `incomplete_reason` 說明缺哪個視窗，並明講推估只計入這台電腦上 Baton 派出的 Run、其他裝置與互動式使用的用量看不到。帳號頁以醒目的提示顯示。
- 理由：CLI 的限流訊號是帳號層級（含其他裝置）的事實，本機推估不是；只要任一視窗只有推估，就可能低估。
- 替代方案：有任何已觀測訊號就算完整（另一個視窗仍可能低估）；永遠標示不完整（失去資訊）。
- 影響：目前真實 Adapter 尚未實作，正式執行時所有帳號都會顯示「觀測不完整」，直到票 14 送出帶數字的訊號。
- 審閱：未審

### AI-125：額度觀測的資料表與保留期限（migration 13）
- 重要度：低
- 時間／階段：2026-10-22，票 27 實作
- 決策：migration 13 `quota`：`quota_observation`（每筆觀測：`origin`＝adapter／estimate／observer、`source`、`quota_window`、`status`、`used`、`tokens`、`limit_tokens`、`reset_at`、`observed_at`、`confidence`、`raw_ref`、`raw`；未知存 NULL）、`quota_usage`（每次用量回報與原文）、`quota_plan_limit`（單一 Profile 一列，含版本）、`quota_poll`（每個 Profile 上次輪詢時間）。每次記下用量、變更方案上限與輪詢時都寫入新的推估觀測。原始資料參照：`run:<run_id>`、`quota_usage:seq=a..b`、`quota_observer`；原文截斷到 2000 字元、不經 API 回傳。Port 失敗記為一筆未知的觀測（原文是錯誤訊息），`NotImplemented` 不留紀錄。輪詢時刪除超過 8 天的觀測與用量，但每個（帳號、來源、視窗）最新的一筆保留。方案上限變更追加事件 `quota_plan_limits_changed`（`aggregate_type = 'quota_plan_limit'`），相同時不寫入。
- 理由：票面要求每筆觀測保存來源、時間、可信度與原始資料參照；每 5 分鐘一筆推估若不刪除會無限成長，8 天涵蓋最長的七日視窗。
- 替代方案：推估即時計算、不落地（沒有「觀測時間」可顯示，也不符合「每筆觀測保存」）；不刪除（資料表無限成長）。
- 影響：本票原本是第 11 個 migration，整合 rebuild/v1（票 12、08 已有第 11、12 個）時順延為第 13 個。時間線（票 41）若需要更久的額度歷史，要調整保留期限。
- 審閱：未審

### AI-126：輪詢的觸發點；票 19 的額度測試改為等下一次輪詢
- 重要度：低
- 時間／階段：2026-10-22，票 27 實作
- 決策：`baton_api::quota::current` 先輪詢到期（距上次 ≥ 5 分鐘或從未輪詢）的使用中帳號再回傳判斷，Supervisor 的每次 tick（新增第 5 步「額度」）、排程佇列、派工 API、排程總覽與容量切換都經過它，所以 Port 每個帳號每 5 分鐘最多被呼叫一次。票 19 的兩個情境測試（`quota_near_the_limit_…`、`quota_blocks_from_seventy_five_percent_…`）原本改了 fake 樣本就立刻生效，現在要先把 fake Clock 前進 5 分鐘。
- 理由：spec D-31 的額度輪詢是 5 分鐘；不再每次排程都呼叫 Port。輪詢時間存在資料庫，API 與 Supervisor 共用、重啟後也延續。
- 替代方案：只在 Supervisor 背景輪詢（harness 不跑背景迴圈，派工 API 會看不到第一次樣本）；每次排程都呼叫 Port（違反 5 分鐘輪詢）。
- 影響：外部觀測器的新樣本最多晚 5 分鐘生效；Adapter 的訊號與用量則立即生效。
- 審閱：未審

### AI-127：額度的 API 與帳號頁形狀
- 重要度：低
- 時間／階段：2026-10-22，票 27 實作
- 決策：`GET /api/v1/scheduling` 的每個帳號新增 `quota`（`used`、`source`、`confidence`、`reset_at`、`observed_at`、`complete`、`incomplete_reason`、兩個視窗各自的 `effective`／`observed`／`estimated`、`external`）與 `plan_limits`，`quota_used` 保留且等於 `quota.used`。新增 `PUT /api/v1/profiles/{id}/quota-limits`（回傳排程總覽；400 `plan_limit_invalid`、404 `profile_not_found`、409 `profile_archived`），Tauri 指令 `profile_set_quota_limits`、`RuntimeClient::set_quota_limits`。帳號頁顯示判斷用量、來源、可信度、重置與觀測時間、兩個視窗的已觀測與推估（推估附本機 token 與方案上限）、觀測不完整提示與方案上限表單，並每 5 分鐘重新讀取排程總覽。外部觀測器的樣本目前不單獨顯示（正式版沒有外部觀測器）。
- 理由：帳號頁已經從排程總覽讀進行中 Run（AI-101），額度放在同一份總覽避免多一個端點；方案上限屬於帳號設定，放在 Profile 路徑下。
- 替代方案：獨立的 `GET /api/v1/quota`（UI 需要合併兩份資料）。
- 影響：Controller 或其他畫面要讀額度時用同一個總覽。
- 審閱：未審

### AI-128：訊息的收件人是同一個 Workspace 的 Task；收件匣跟著 Task，不跟著 Run
- 重要度：中
- 時間／階段：2026-10-22，票 18 實作
- 決策：`send_message` 的 `to` 是同一個 Workspace 內其他 Task 的 id（1 到 10 個，去除重複，不能含自己的 Task）；收件 Task 不必有進行中的 Run。每個收件 Task 一筆交付（主鍵 workspace＋message_id＋收件 Task）。`read_inbox` 只回傳寄給「憑證所屬 Run 的 Task」的訊息：交接或重試後的新 Run（新世代）會接到前一個 Run 沒讀完的訊息；自己寄出的訊息不在自己的收件匣。收件 Task 不存在或屬於別的 Workspace 一律回 404 `recipient_not_found`，不洩漏別的 Workspace 是否有那個 Task。`in_reply_to` 只能指向寄件者自己寄過或收到的訊息，否則回 404 `reply_target_not_found`。Controller 與使用者目前不是收件人（Controller 的訊息通道由後續票處理）。
- 理由：Task 才是跨 Run 穩定的歸屬單位（glossary：交接會結束舊 Run 並建立 successor Run，兩者屬同一個 Task）；若收件匣綁 Run，交接時未讀訊息會遺失。Worker 不知道別人的 run id，用 Task id 也和 `get_task_contract` 回傳的 `task_id` 一致。
- 替代方案：收件人用 run id（交接就遺失訊息、也要求收件人已經在跑）；廣播給整個 Workspace（Worker 會被無關訊息淹沒，也更難界定「自己範圍」）。
- 影響：Worker 要知道對方的 task id（來源：Prompt 或 Controller 提供）；本票沒有「列出同 Workspace 的 Task」工具，由 Controller 在 Prompt 中提供。收件 Task 已完成或取消時訊息仍會存入但不會被讀到。
- 審閱：未審

### AI-129：`message_id` 是寄件者提供的冪等鍵，範圍是 Workspace
- 重要度：中
- 時間／階段：2026-10-22，票 18 實作
- 決策：`message_id` 為選填（字元 `A-Za-z0-9_-.:`、1 到 128）。有提供時它就是去重鍵，唯一範圍是（Workspace、message_id）：同一個寄件 Task 以同一個 `message_id` 重送完全相同的內容（類型、主旨、本文、`in_reply_to`、收件人集合）→ 200 `duplicate: true`，回傳原本的訊息，不新增訊息或交付，寄件 Run 換世代重送也算；同一個 `message_id` 但內容不同、或是別的 Task 使用 → 409 `message_id_conflict`。沒有提供時 Runtime 產生 `msg_…`（這種請求每次都是新訊息，沒有重送保護）。不同 Workspace 可以使用相同的 `message_id`，彼此看不到對方。`thread_id` 由 Runtime 決定（新訊息＝自己的 `message_id`，回覆＝沿用被回覆訊息的 `thread_id`），`correlation_id` 可由寄件者指定，預設沿用被回覆訊息的、否則等於 `message_id`。
- 理由：驗收條件「重複送出的訊息只交付一次」與「每個收件人以 message_id 去重」；Worker 重試工具呼叫（逾時、shim 重送）時需要穩定的冪等鍵，Runtime 無法替沒有鍵的請求猜測是不是重送。資料庫層以（workspace、message_id、收件 Task）主鍵保證同一則訊息對同一個收件人不可能有兩份。Workspace 為範圍與整個平台「所有範圍查詢都以 Workspace 為界」一致，也不會因 id 衝突洩漏別的 Workspace 的資訊。
- 替代方案：全域唯一的 `message_id`（跨 Workspace 的衝突會洩漏存在性）；以內容雜湊自動去重（合法的連續相同訊息會被吞掉）；不允許 Worker 指定 `thread_id`（採用，避免加入別人的對話串）。
- 影響：工具說明建議 Worker 重送時帶同一個 `message_id`。寄件 Task 相同但內容不同的重送會得到 409，而不是悄悄覆蓋。
- 審閱：未審

### AI-130：「以訊息變更範圍」的判定方式與處理：看類型與欄位名稱，200 忽略並轉成事件
- 重要度：高
- 時間／階段：2026-10-22，票 18 實作
- 決策：訊息本文是自由文字，Runtime 不解讀語意。被視為「想改變範圍或歸屬」的只有兩種明確訊號：`kind` 為 `scope_change`、`reassign`、`reassign_task`、`change_scope`、`update_contract`，或請求中出現 `task_id`、`contract`、`scope`、`assignee`、`assign_to`、`reassign_to`、`new_scope`、`owner` 任一欄位。這類請求：不儲存、不交付，HTTP 200 回 `{accepted:false, ignored:true, reason:"scope_change_not_allowed"}`，並追加一筆事件 `message_scope_change_ignored`（aggregate_type `message`，payload 含 workspace、run、寄件 Task、世代、嘗試的類型與欄位、收件人、主旨、本文前 500 字元）給 Controller。寄件 Run 已不是目前的寫入所有者時同樣回 409 `run_report_stale`，不記事件。正常訊息的合法類型只有 `note`、`question`、`answer`、`result`，其他值 400 `message_invalid`。訊息工具沒有任何路徑能修改 Task、契約、Run 或授權：訊息只寫 `message`、`message_delivery` 兩張表。
- 理由：驗收條件要求「嘗試以訊息變更範圍時被忽略並轉為給 Controller 的事件」。從本文判斷語意不可靠（誤判合法協作、漏判改寫措辭），所以只處理結構上明確的嘗試；其餘訊息即使文字上在要求改範圍，也只是收件 Worker 讀到的文字，不會改變任何契約（範圍的唯一來源仍是 Task 契約，ADR-0011）。用 200 而不是錯誤，是讓 Worker 知道「沒有被執行」但不會把它當作故障重試。
- 替代方案：用 4xx 拒絕（Worker 可能當成暫時性錯誤重試，而且不會產生 Controller 事件）；對本文做關鍵字偵測（誤判率高）；把嘗試存成一般訊息但標記（收件人仍會讀到，違反「被忽略」）。
- 影響：Controller 消費 `message_scope_change_ignored` 事件的邏輯由 Controller 相關的票實作；本票只保證事件存在且內容足夠。訊息本文本身若含有誘導收件 Worker 超出範圍的文字，防護是 Worker 的契約與授權檢查，不在本票。
- 審閱：未審

### AI-131：交付狀態、Run 啟動時的未讀摘要與「不中途插話」
- 重要度：中
- 時間／階段：2026-10-22，票 18 實作
- 決策：每筆交付有三種狀態：`pending`（已存入，收件人還沒看到）、`delivered`（出現在某個 Run 啟動時的未讀摘要裡）、`read`（收件人用 `read_inbox` 取走全文；`read_inbox` 直接讀取尚未 delivered 的訊息時一併補上 delivered 時間）。狀態由 `delivered_at`、`read_at` 兩個時間推導，不另存狀態欄。派工當下（`apply_run_command` 處理 Dispatch 的同一個交易）把收件 Task 的未讀訊息摘要附在 Prompt 末端：第一行 `未讀訊息：N 則（…）`，之後每則 `- [類型] 主旨（來自 Task「標題」，message_id）`，最多 10 則，超過加一行 `…另有 M 則`，**不含本文**；摘要列出的訊息標為 delivered；沒有未讀時 Prompt 不變。Prompt 在派工時固定並存入 Outbox（Runtime 重啟後重組出相同指令），所以派工之後才送來的訊息只能靠 `read_inbox` 取得；Runtime 不在 Worker 執行中途注入。`read_inbox` 預設只回未讀（依送出順序、最多 `limit`，預設 50、上限 100）並標為已讀；`include_read: true` 連已讀的一起回傳（每則有 `already_read`）；`unread_remaining` 是讀完之後仍未讀的數量。
- 理由：D-27 規定拉取式交付與「Context Builder 在每次 Run 啟動時附上未讀摘要」。Context Builder 尚未存在（`render_prompt` 仍是最小版本，AI-73），所以摘要在 Store 派工交易內附加，與派工的其他寫入同一個交易、也和 Prompt 同時固定；Context Builder 加入後把這段納入它即可。摘要不含本文以免大量訊息撐大 Prompt，也避免把不受信任的訊息文字直接放進 Prompt 的指令區。
- 替代方案：啟動時（Supervisor 啟動程序那一刻）才重新計算摘要（Prompt 不再固定，與「重啟後重組相同指令」衝突）；讀取時只記單一 `read` 狀態（無法區分「已提示」與「已取走」）。
- 影響：派工到實際啟動之間送達的訊息不在摘要裡，Worker 開始工作前仍應呼叫 `read_inbox`（工具說明這麼寫）。派工交易需要查詢訊息表，多一次小查詢。
- 審閱：未審

### AI-132：訊息的其他限制與資料表命名
- 重要度：低
- 時間／階段：2026-10-22，票 18 實作
- 決策：資料表命名沿用單數慣例：`message`（追加式，禁止更新與刪除）、`message_delivery`（禁止刪除，只會填入 `delivered_at`、`read_at`），不照票文的複數 `messages`／`message_deliveries`。限制：主旨 1 到 200 字元、本文最多 16 KiB、收件人最多 10 個。`send_message` 只接受目前仍是寫入所有者的 Run（已結束或被接替 → 409 `run_report_stale`，什麼都不儲存，也不留稽核）；`read_inbox` 在 Run 結束或被接替後 401 `credential_revoked` 且不改變已讀狀態；兩者都沿用「請求聲稱的 Workspace／Run 與憑證不一致 → 403 `agent_scope_mismatch`」。`send_message` 不續約 lease（只有回報進度、checkpoint、受阻才是存活訊號）。`read_inbox` 的 `include_read` 搭配 `limit` 時回傳最舊的幾則。
- 理由：與既有資料表命名一致（`agent_report`、`event`、`run`）；上限與 Worker 回報的上限同量級，避免訊息成為塞大量資料的通道。寄出的訊息不是 Worker 在「做事」的證明，不該延長 lease；被接替的 Run 遲到的訊息沒有決策價值，所以不像回報那樣留稽核。
- 替代方案：照票文用複數表名；過期世代的訊息也存稽核（會讓舊 Worker 的內容留在別人的收件匣旁邊，沒有明確用途）。
- 影響：若之後需要稽核被拒絕的訊息，需另開票加入。
- 審閱：未審

### AI-133：票 18 整合：migration 與 AI 編號順延到票 19 之後；排程派工也附上未讀摘要
- 重要度：中
- 時間／階段：2026-10-22，票 18 整合（R037-18-integrate）
- 決策：整合 rebuild/v1（票 19 已合併）時，票 19 的 `scheduling` 維持第 10 個 migration，本票的 `message_and_delivery` 順延為第 11 個（排在 `scheduling` 之後；R048 再順延為第 14 個，見 AI-134）；本票的決策由 AI-95～AI-99 順延為 AI-103～AI-107（R048 再順延為 AI-128～AI-132，見 AI-134），移到 AI-102 之後並補上審閱行，內容不變。兩個 migration 互不相依（訊息表只參照 `workspace`、`run`、`task`，不涉及 `run.profile_id`），先後順序不影響 schema。票 19 之後派工 API 與 Supervisor 都改走 `Store::schedule`，不再經過 `apply_run_command`，所以在 `schedule` 派出 Run 的同一個交易裡也呼叫 `attach_unread_summary`，維持 AI-131 的「Run 啟動時附上未讀摘要」；`apply_run_command` 的呼叫保留（仍可直接派工）。本票的 harness 情境測試比照票 19 先建立可派工帳號；因單一帳號並行上限是 2，而部分情境同時有 3 個 Run，所以建立兩個帳號；Store 測試的 `RunCommand::Dispatch` 補上 `profile_id`。測試的斷言都沒有改。
- 理由：整合分支的條目與 migration 編號不能變動；本票尚未發布，順延不影響任何既有資料庫。
- 替代方案：讓本票保留第 10 個、把票 19 順延（會改動整合分支已存在的 migration 版本，已套用的開發資料庫版本號會錯位）。
- 影響：本票程式碼與文件沒有引用 AI-95～AI-99 或 migration 編號，不需其他同步修改。只套用到第 10 版的資料庫開啟時會補套後續版本。之後若再新增派工路徑，要記得同樣附上未讀摘要（Context Builder 加入後應收斂到一處）。
- 審閱：未審

### AI-134：票 18 第二次整合 rebuild/v1：migration 第 14 版、AI 編號再順延
- 重要度：中
- 時間／階段：2026-10-22，票 18 整合（R048-18-integrate）
- 決策：rebuild/v1 已合併票 12（`artifact`，第 11 版）、票 08（`shared_settings_version`，第 12 版）、票 27（`quota`，第 13 版），本票的 `message_and_delivery` 順延為第 14 個 migration，排在 `quota` 之後；本票的決策由 AI-103～AI-108 順延為 AI-128～AI-133，移到 AI-127 之後，內容不變，只在 AI-133 的歷史說明加註再次順延，並把其中對 AI-106 的引用改為 AI-131。衝突合併方式：Agent 工具變成 7 個（原有 4 個＋票 12 的 `register_artifact`＋本票的 `send_message`、`read_inbox`），shim 的工具定義、已知工具清單與 API 路由三者都保留；shim 情境測試把兩邊的 schema 斷言放在同一個測試（改名為 `tools_list_has_the_seven_tools_with_their_required_fields`），stdio 測試的工具數改為 7；GLOSSARY 兩邊新增的詞條都保留。
- 理由：整合分支的條目與 migration 編號不能變動；本票尚未發布，順延不影響任何既有資料庫。訊息表只參照 `workspace`、`run`、`task`，與成果、共同設定、額度的資料表互不相依，順序不影響 schema。
- 替代方案：把本票的 migration 插在中間（會改動整合分支已存在的版本號，已套用的開發資料庫會錯位）。
- 影響：本票程式碼與文件沒有引用 AI 編號或 migration 編號，不需其他同步修改。工具數量相關的斷言之後若再新增工具，要同時更新兩個 shim 測試。
- 審閱：未審

### AI-137：子代理建置關閉增量編譯與除錯資訊，閒置 worktree 的 target 隨時可刪
- 重要度：中
- 時間／階段：2026-10-22，票 13、14、22、25、37 進行中
- 決策：`dispatch.sh` 為每個 run 設定 `CARGO_INCREMENTAL=0`、`CARGO_PROFILE_DEV_DEBUG=0`（只影響子代理的環境，不改 repo 的 Cargo.toml）。沒有 run 在執行的 worktree，主控可直接刪除其 `target/`；票結案時 close.sh 已會移除整個 worktree。
- 理由：每個 worktree 各自編一份含完整除錯資訊與增量快取的 debug 版，單一 worktree 的 `target/debug` 達 32.8 GB，6 個 worktree 合計約 143 GB，C 槽一度歸零，R050、R051、R052 的建置都因 `os error 112` 失敗過。增量快取與除錯資訊是體積的大宗，對一次性的 run 沒有用處。
- 替代方案：所有 worktree 共用一個 `CARGO_TARGET_DIR`（體積最小，但 cargo 會以檔案鎖讓並行建置排隊）；或每個 run 結束就 `cargo clean`（下一個 run 要整套重編）。
- 影響：測試失敗時的 backtrace 只有函式名稱、沒有行號；每次建置都是非增量，第一次編譯時間不變、反覆修改時較慢。
- 審閱：未審

### AI-138：權限提示的判定是保守的詞法判定；範圍外或看不懂一律建立裁定請求
- 重要度：高
- 時間／階段：2026-10-22，票 22 實作
- 決策：`permission_prompt` 先把「工具名稱＋輸入」歸類成 Workspace 授權的能力（`baton_domain::permission`），再對照**目前生效（最新）的授權 revision**：(1) 檔案工具（Read／Glob／Grep／LS → 讀取；Edit／MultiEdit／Write／NotebookEdit → 編輯）的路徑必須落在該能力的授權目標目錄內（詞法正規化：不分大小寫、斜線方向、`.`／`..`；相對路徑以 Workspace 目錄為基準；`..` 逃出根目錄、`~` 開頭、UNC 與其他磁碟都算範圍外），寫入 `.git` 一律範圍外（授權限制「不修改 Git 歷史」）；(2) WebFetch 只允許公開 http／https（本機、內網、IPv6 字面位址都算範圍外），WebSearch 需要「讀取公開網頁」；(3) Bash 只認一小張白名單（`cargo`／`npm`／`pnpm`／`yarn`／`pytest`／`go`／`dotnet` 的建置、測試、安裝子命令，`git status|diff|log|show`、`git add|commit`、`git push`），含任何 shell 組合語法（`; & | < > \` $ ( ) { } * ?`、換行）的指令、未知子命令、不認得的程式一律無法歸類；指令以 bash 規則斷詞（去掉引號；引號不成對、反斜線緊接引號一律無法歸類；判斷旗標前去掉反斜線，所以 `'--amend'`、`\-\-amend` 都認得），**每個子命令只接受白名單內的旗標**，其他旗標一律無法歸類——例如 `git commit --amend|--fixup|--squash|-C|--reset-author`、`git diff|log|status --output|-o`、`git -C …`、`git push --force` 與 `+分支`／`:分支` refspec、`npm|pnpm|yarn|bun --prefix|-C|--dir`、`cargo --manifest-path|-Z|--config|--target-dir`；帶值旗標的值不可以 `-` 開頭；`-g`、`--global`、`--global=…`、`-g=…`、`--location=global`、`--location global` 一律歸為「全域安裝」（`--global=false` 也是，保守處理）；指令參數中像路徑的字詞（含 `/`、`\`、磁碟代號或以 `.`、`~` 開頭）必須在 Workspace 目錄內（檔案能力則對照授權目錄），否則範圍外；(4) 未知工具、缺少必要欄位、尚未確認任何授權，全部範圍外；(5) 路徑元件比對前依 Win32 規則去掉結尾的 `.` 與空白（`.git.`、`.git ` 視同 `.git`）；只剩點與空白的元件、磁碟代號以外含 `:` 的元件（NTFS 替代資料流，例如 `.git::$INDEX_ALLOCATION`）、`\\?\`／`\\.\` 裝置路徑都視為無法正規化而算範圍外；8.3 短檔名 `GIT~n` 視同 `.git`。範圍外建立裁定請求並回「等待裁定」，不是回傳錯誤。（第 (3) 項的旗標白名單與第 (5) 項由修正 run R041 依驗證報告 R038 補上。）
- 理由：D-08 要求「授權範圍內自動允許並記錄，範圍外建立裁定請求」，而且 Worker 可能被提示注入；把「看不懂」當成範圍外才不會讓組合指令（`cargo test && curl … | sh`）借白名單前綴溜進去。用最新 revision 而不是 Run 准入時的 revision，是因為使用者收窄授權後，仍在跑的 Worker 不應該繼續沿用舊的寬鬆授權。
- 替代方案：真的解析 shell 語法（Windows 的 cmd、PowerShell、bash 語法都不同，錯一個就是漏洞）；用 Run 准入時固定的 revision（擴權安全、收權不安全）；把無法歸類的操作直接拒絕而不問使用者（Worker 卡住且使用者看不到任何東西）。
- 影響：保守的代價是 Worker 常用的 `cargo test 2>&1 | tail`、`ls`、`cat`、`curl` 這類指令每次都會變成裁定請求；使用者可用「允許這一次」，或日後把常見指令加進白名單／在 Adapter 的預先允許清單（票 14）列出。詞法判定不解析符號連結，授權目錄內指向外部的連結不會在這裡被攔下（Workspace 目錄本來就不是安全邊界，ADR-0002）。Adapter 預先允許的工具清單與 `--permission-prompt-tool` 參數屬票 14，這裡只定義 Runtime 端的判定。
- 審閱：未審

### AI-139：範圍外的權限提示只有兩個選項；「允許這一次」是單次批准，且由同一 Task 的同一操作消耗一次
- 重要度：高
- 時間／階段：2026-10-22，票 22 實作
- 決策：權限提示建立的裁定請求固定兩個選項 `allow_once`（允許這一次）與 `deny`（拒絕），**沒有**「永遠允許」或「擴大授權」選項；高風險能力或無法歸類的操作建議 `deny`，其餘不給建議。使用者回答之後，**同一個 Task** 再次提出**同一個操作**（工具名稱＋輸入的指紋相同）時，這次提示消耗那個批准：`allow_once` → 回 allow（`status: approved`），`deny` → 回 deny（`status: denied`），消耗紀錄存在追加式的 `decision_approval_use`，每個請求最多消耗一次；消耗之後再提示同一個操作要重新裁定。同一個 Task 對同一個操作在請求仍待處理時重複提示，回到既有請求（不重複建立）。
- 理由：回答權限提示若沒有人消耗答案，Task 恢復後 Worker 會再被問到一次、無限循環；單次批准對應 spec 的「E1 單次批准」，也維持「Agent 不能擴大授權」。把 `deny` 也消耗一次，讓 Worker 能得到明確的拒絕理由，而不是又被無限期擱置。
- 替代方案：回答後永遠記住（等於偷偷擴大授權，且沒有失效機制）；只記錄答案、不消耗（Worker 無法得知結果）；批准綁 Run 而不是 Task（Task 恢復後一定是新的 Run，批准會永遠用不到）。
- 影響：批准以 Task 為界，Task 取消後批准自然失效；擴大 Workspace 授權仍只能由使用者在授權設定頁做。使用者想永久放行某類操作要去改授權。
- 審閱：未審

### AI-140：裁定請求對 Task 與 Run 的影響：只有 `running` 的 Task 進入 `waiting_decision`，Run 不結束；恢復條件是「沒有其他待處理請求」
- 重要度：中
- 時間／階段：2026-10-22，票 22 實作
- 決策：(1) 請求建立時，只有提出請求的 Run 所屬的 Task 在 `running` 時才轉成 `waiting_decision`（`blocking` 欄位記下受影響的 Task，Agent 提出的請求目前永遠只有那一個 Task）；Task 已是別的狀態（例如 `blocked`）時請求照常建立，Task 不動。(2) **Run 不因此結束**（與 AI-90 的 `report_blocked` 一致）：Worker 依 D-08 保存 checkpoint 後自行結束，或繼續做不需要這項裁定的事；Run 結束時 Task 不在 `running`，所以不會被拉到 `verifying` 或 `ready`。(3) 回答時，每個 `blocking` 的 Task 只有在「仍是 `waiting_decision`，而且沒有任何其他待處理請求也阻擋它」時才回到 `ready`；已取消或已離開 `waiting_decision` 的 Task 不動。(4) 只有仍是 execution lease 持有者的 Run 能提出請求（沿用 `is_current_writer`），已結束或被接替的 Run 提出請求回 409 `run_report_stale`，不建立任何東西。(5) 回到 `ready` 的 Task 在**同一個交易**重新排進派工佇列（`dispatch_request`，優先序 0、`requested_at` 是裁定時間；已在佇列中的保留原位置），之後由 Scheduler 派出：Supervisor 每次處理都會排程佇列，回答的 API 也會立刻叫醒 Supervisor。舊 Run 若還活著、還持有 execution lease，排程判定為 `execution_conflict`，Task 留在佇列（等待原因歸「狀態不明」）而不是被丟掉；舊 Run 結束後的下一次排程派出新的 Run。不必有人再按派工。
- 理由：D-08／使用者故事 55 的「恢復排程」＝Task 回到 `ready` 並交回 Scheduler，而不是由裁定直接啟動程序（程序只能由 Supervisor 啟動，I3；准入與容量檢查仍由 Scheduler 做）。票 19 的 Scheduler 只處理佇列內的 Task、派出時就移出佇列，所以只把 Task 改成 `ready` 不會被自動派出，必須重新排隊（原本「由票 19 的 Scheduler 看到 `ready` 後處理」的前提與實作不符，由修正 run R041 依驗證報告 R038 改正）。多個請求阻擋同一個 Task 時只回答其中一個就恢復，會讓 Worker 帶著未解決的問題重新開工。
- 替代方案：裁定時直接派工（繞過准入與容量檢查）；Run 提出請求時一併結束 Run（Worker 要做收尾，Domain 沒有「Worker 自述結束」的路徑）；每個請求獨立恢復 Task（上述問題）；讓 Scheduler 自動派出所有 `ready` 的 Task（會讓失敗的 Run 無限重派，違反 AI-96）。
- 影響：`waiting_decision` 的 Task 在使用者沒有回答之前不會有任何出口（取消除外）；回答之後自動恢復，是 AI-96「Task 回到 `ready` 不會自動重新排隊」的唯一例外。原本派工的優先序沒有保存，恢復時一律用預設值 0。Controller 專屬的 `propose_decision_request`（可阻擋多個 Task）屬票 16、17，Domain 的 `blocking` 已經是清單，不需要改資料模型。
- 審閱：未審

### AI-141：逾時不是同意——期限只標示「已逾期」，沒有任何因時間而發生的狀態變化
- 重要度：高
- 時間／階段：2026-10-22，票 22 實作
- 決策：`request_decision` 可帶 `deadline_minutes`（1 分鐘到 30 天，從 Runtime 的 Clock 起算，存成 UTC 時間）；請求的投影帶 `overdue`（待處理且現在超過期限）。**沒有任何命令或背景處理會因為時間流逝而回答請求、放行權限提示或讓 Task 恢復**；逾期後使用者仍可回答，回答才會恢復 Task。權限提示建立的請求沒有期限。測試涵蓋：推進 fake Clock 數天、重啟 Runtime、再次 supervise，請求仍是 pending、Task 仍是 `waiting_decision`；同一個權限提示放三天後仍回「等待裁定」。
- 理由：使用者故事 55「我的沉默不會變成授權」是安全邊界，不是可調參數；把「逾期」做成純顯示欄位，可以保證沒有程式路徑能把時間當成同意。
- 替代方案：逾期自動採用建議選項或自動拒絕（前者是沉默即同意；後者會讓 Task 無人知曉地失敗，也需要新的狀態）；逾期把請求標為 expired（spec 的 expired／superseded 屬完整裁定中心，見 AI-143）。
- 影響：Agent 看不到使用者的期限壓力以外的任何回饋；使用者錯過的請求會一直留在待處理清單（以緊急程度與建立時間排序，逾期者有標示）。
- 審閱：未審

### AI-142：裁定的資料結構、交易邊界與喚醒 Outbox
- 重要度：中
- 時間／階段：2026-10-22，票 22 實作
- 決策：(1) migration 15（`decision_request_and_decision`，排在票 18 的 `message_and_delivery`（migration 14）之後）新增 `decision_request`（只能從 `pending` 改成 `answered`，其餘欄位由觸發器鎖住、不可刪除）、`decision_request_block`（阻擋範圍）、`decision`（追加式，`decided_by` 以 CHECK 限定為 `desktop`，所以連程式寫錯也寫不進非桌面的裁定）、`decision_approval_use`（追加式）。(2) 回答在**同一個交易**寫入裁定、請求狀態、受影響 Task 的狀態與事件、喚醒 Outbox。(3) 喚醒 Outbox 的 `kind` 是 `decision_wake:<請求識別>`（Outbox 的唯一鍵是 `(kind, run_id)`，同一個 Run 可以提出多個請求，所以種類帶上請求識別；沒有改既有資料表），`run_id` 是提出請求的 Run，內容有 Workspace、請求、選項、阻擋的與實際恢復的 Task。**本票沒有消費者**：Controller 的事件迴圈（票 16 之後）會讀它，`delivered_at` 保持空白。(4) 事件：請求聚合 `decision_request`（v1 `decision_requested`、v2 `decision_answered`、v3 `permission_approval_used`），自動允許的權限提示每筆一個獨立聚合 `permission_audit`（`permission_auto_allowed`，內容含 Run、Task、工具、摘要、能力與授權 revision；不節流）。(5) 回答同樣的選項重送是冪等的（200、不再追加任何東西）；不同的選項回 409 `decision_already_answered`；未知選項 400 `decision_option_unknown`；請求內容無效 400 `decision_invalid`。
- 理由：「裁定已存、Task 還卡住」的中間狀態不能被觀察到（驗收條件）；不改 Outbox 的唯一鍵可避免與同時進行的票互相衝突；稽核事件與狀態變更同交易才能保證記錄與實際行為一致。
- 替代方案：用新的 `wake` 資料表（多一張沒有消費者的表）；重建 Outbox 表移除唯一鍵（動到其他票也在使用的表）；自動允許只記日誌（日誌可被輪替、不在備份範圍，也不能和 Run 關聯查詢）。
- 影響：自動允許每次都追加事件，讀取密集的 Worker 會讓事件表成長較快，需要之後的清理／彙總策略（與事件表清理一併處理）；`decision_wake` 項目在 Controller 票實作前會累積。
- 審閱：未審

### AI-143：本票的裁定中心只做最小版本，其餘留給「裁定中心完整功能」
- 重要度：中
- 時間／階段：2026-10-22，票 22 實作
- 決策：本票做：Agent 以 `request_decision` 提出、權限提示轉接、待處理清單與單筆詳情、單筆回答、受影響 Task 恢復、喚醒 Outbox、狀態列與導覽的待處理數。**不做**（spec 第三階段的完整裁定中心）：`expected_revision` 校驗與 `superseded`／過期標記、批次回答、相似請求合併或互相連結、`goal_revision`、決策權限表（D-15）的強制檢查、Decision 的適用範圍／有效期間／supersedes、Controller 專屬的 `propose_decision_request`。選項限制：2–8 個、識別限英數字與底線連字號；問題 500、原因 1000、選項名稱 200、取捨 500、證據最多 10 項各 500 字元、備註 1000 字元。Agent 工具另外去重：同一個 Run 對同一個問題在仍待處理時重複提出，回到既有請求並標示 `existing: true`。
- 理由：票面與 spec 第 214 行都限定第一階段只需「建立請求、顯示、單筆回答、喚醒工作」；長度與選項數量的上限避免 Agent 用超大內容塞滿裁定畫面。
- 替代方案：一次做完整裁定中心（範圍遠超出本票，且依賴尚未存在的 Goal revision、Context Builder）。
- 影響：Decision 目前只保存「選了哪個選項與備註」，Context Builder（票 15、28）要載入決策時從 `decision` 與請求讀取；完整版會在同一組資料表上追加欄位或新表，不需要重建。
- 審閱：未審

### AI-144：Agent 工具與權限提示回應的形狀（遵循 Claude Code 的 permission-prompt 協定，待票 14 驗證）
- 重要度：中
- 時間／階段：2026-10-22，票 22 實作
- 決策：新增兩個 Agent 工具：`request_decision`（欄位 `question`、`reason`、`options[{id,label,tradeoff?}]`、`recommendation?`、`evidence?`、`urgency?`、`deadline_minutes?`；回 `{decision_request_id, status: "waiting_decision", existing, message}`）與 `permission_prompt`（欄位 `tool_name`、`input`，可另帶 `tool_use_id`；回 `{behavior: "allow"|"deny", updatedInput?, message?, status, decision_request_id?, authorization_revision?}`）。回應遵循 Claude Code `--permission-prompt-tool` 的形狀（`behavior`／`updatedInput`／`message`），允許時原樣帶回輸入；等待裁定與拒絕都是 HTTP 200 的 `behavior: "deny"`（MCP 結果不是 `isError`），`status` 區分 `auto_allowed`、`approved`、`waiting_decision`、`denied`。Agent API 沒有任何回答或批准的工具；桌面 API 一律在 `/api/v1/` 下，Agent 憑證被 guard 擋成 403 `role_not_allowed`。桌面端點：`GET workspaces/{id}/decisions`、`GET …/decisions/{rid}`、`POST …/decisions/{rid}/answer`（`{option_id, note?}`）、全域 `GET /api/v1/decisions/pending`。
- 理由：沿用已知協定可讓票 14 的 Adapter 直接把 `permission_prompt` 指給 `--permission-prompt-tool`；`deny` 不用 `isError` 是因為它是正常的協定回覆，不是工具故障。
- 替代方案：自訂回應格式再由 shim 轉換（多一層轉換就多一處可能不一致）。
- 影響：**Validation Required（票 14，以真實 Claude Code 驗證）**：(a) CLI 是否接受 `behavior` 之外的額外欄位、是否要求 `updatedInput` 一定存在；(b) 收到 `deny` 的 Worker 是否真的能繼續做其他步驟而不重試相同操作；(c) 被阻擋的權限提示是否需要 `tool_use_id` 才能對應。若與協定衝突，只需要修改 `PermissionResponse` 的序列化，Domain 判定不受影響。前端的 Tauri 指令名稱（`decision_list`／`decision_detail`／`decision_answer`／`decisions_pending`）與 HTTP 路徑一一對應。
- 審閱：未審

### AI-145：票 22 整合 rebuild/v1（票 12、08、27）：migration 與 AI 編號順延、衝突合併方式
- 重要度：低
- 時間／階段：2026-10-22，票 22 實作（修正 run R041）
- 決策：整合分支在本票開發期間先後併入票 12（`artifact`，migration 11）、票 08（`shared_settings_version`，migration 12）與票 27（`quota`，migration 13），所以票 22 的 migration `decision_request_and_decision` 由第 11 個順延為第 14 個，排在最後；彼此互不參照。票 22 的 AI 編號由 AI-103～AI-109 順延為 AI-138～AI-144，修正 run 新增的條目是 AI-145、AI-146；內容不變，開發指南的引用同步改號；整合分支既有條目不動。其餘衝突一律兩邊都保留：API 路由、Tauri 指令、MCP 工具清單（共 7 個）、`AppClient` 的各組用戶端、側欄入口（「裁定」與「成果與驗證」並列）、開發指南的 crate 表與測試清單。
- 理由：不可修改整合分支已有的 migration 與 AI 條目；這幾張票的功能彼此獨立。
- 替代方案：合併 migration（已套用的開發資料庫會漏掉其中一部分）。
- 影響：無行為變更。
- 審閱：未審

### AI-146：修正 R038 的判斷：以 bash 斷詞、旗標白名單擴及全部指令、指令參數的路徑檢查、恢復時的優先序
- 重要度：中
- 時間／階段：2026-10-22，票 22 實作（修正 run R041）
- 決策：(1) 權限提示的指令以 **bash** 規則斷詞（Claude Code 在 Windows 上的 Bash 工具由 Git Bash 執行）；各 shell 解讀不一致的寫法（反斜線緊接引號、引號不成對）一律無法歸類，而不是猜一種解讀。(2) 驗證報告只點名 git、npm 家族、cargo；同樣的「旗標白名單」也套用到 pip、pytest、tsc、vitest、jest、go、dotnet，因為它們有同類的旗標（`go build -o`、`tsc --outDir`、`pip --target`）。(3) 指令參數裡像路徑的字詞必須在 Workspace 目錄內（`pytest C:\other`、`npm install ../外部套件`）；commit 訊息、測試篩選這類自由文字旗標的值不檢查。(4) 寫入時 `GIT~n`（`.git` 的 8.3 短檔名）視同 `.git`。(5) 裁定後重新排隊的優先序一律是 0（原本派工的優先序沒有保存；要保存得在 `run` 或 Task 加欄位，超出本次修正）。(6) AI-138、AI-140 是本票自己尚未審閱的條目，依驗證報告的要求直接修正內容並註明來源，不另開取代條目。
- 理由：保守判定的原則（AI-138）是「看不懂就問」；只修報告點名的旗標，同類的繞過仍會存在。
- 替代方案：只修報告點名的項目（其他工具有一樣的漏洞）；自行完整解析 bash、cmd、PowerShell 語法（錯一個就是漏洞）；保存原本派工的優先序（要改資料表）。
- 影響：白名單外的常用旗標（例如 `git log --format=…`、`pnpm --filter web test`、`cargo +nightly`）現在也會變成裁定請求；使用者可「允許這一次」，常用的再逐步加進白名單。
- 審閱：未審

### AI-147：票 22 整合 rebuild/v1（票 18）：migration 第 15 版、AI 編號再順延、MCP 工具合成 9 個
- 重要度：低
- 時間／階段：2026-10-22，票 22 整合（run R053-22-integrate）
- 決策：整合分支先併入票 18，佔用了 migration 14（`message_and_delivery`）與 AI-128～AI-134，另有 AI-137；所以票 22 的 migration `decision_request_and_decision` 由第 14 個順延為第 15 個，排在最後，兩者互不參照、都會套用。票 22 的 AI-128～AI-136 順延為 AI-138～AI-146（內容只改編號與 migration 序號），開發指南的引用同步改號；整合分支既有條目不動。其餘衝突一律兩邊都保留：Domain 的工具常數與模組、Store 的模組與匯出、API 路由（`request_decision`、`permission_prompt`、`send_message`、`read_inbox` 並存）、MCP shim 的工具定義與已知工具清單（共 9 個），shim 情境測試兩邊的 schema 斷言與轉送測試都保留，工具數的斷言由 7 改為 9（測試名稱改為 `tools_list_has_the_nine_tools_with_their_required_fields`）。
- 理由：不可修改整合分支已有的 migration 與 AI 條目；兩張票的功能彼此獨立。
- 替代方案：合併 migration（已套用的開發資料庫會漏掉其中一部分）；把票 22 的條目放在 AI-135、AI-136 的空號（整合分支跳過這兩號的原因不明，且本票有 9 個條目放不下）。
- 影響：無行為變更。
- 審閱：未審

### AI-148：確定性檢查放在契約內，第一版三種：命令結束碼、工作目錄內檔案、Task 成果
- 重要度：高
- 時間／階段：2026-10-22，票 13 實作
- 決策：契約內容新增 `verification_checks`（與 `required_capabilities` 一樣隨契約版本保存，屬「完成與驗證條件」的機械可判定部分），最多 20 項，三種：(1) `command`：在受驗 Run 的工作目錄執行（不經過 shell），結束碼等於 `expected_exit_code`（預設 0）；(2) `file`：工作目錄內的相對路徑是存在的檔案，可選 `sha256`；(3) `artifact`：這個 Task 有這個名稱的成果，最新版本的檔案與 manifest 雜湊一致，可選 `sha256`。儲存契約時驗證（路徑不可含 `..`、磁碟代號、從根目錄開始；雜湊必須是 64 個十六進位字元並轉成小寫；成果名稱沿用成果登記的檔名規則），不合規回 400 `contract_invalid`。沒有宣告任何檢查不影響准入（`done_criteria` 文字仍必填），這種 Task 只能由使用者手動接受。Worker 的提示（`render_prompt`）會列出宣告的檢查。命令逾時 10 分鐘、被終止、無法啟動、檔案讀不到，一律判定「未通過」，不另分「環境故障」。
- 理由：票面要求「契約中宣告的確定性檢查（命令結束碼、檔案存在與雜湊等）」；放進契約才有版本，Worker 也無法自行改寫完成標準（ADR-0011）。成果檢查讓「檔案存在與雜湊」也能套用在研究與文件這類沒有工作目錄產出的 Task。
- 替代方案：把檢查寫成 `done_criteria` 的自由文字再由模型判讀（不確定性，違反「不採信 Worker 說法」）；另開一張「驗證設定」表（與契約版本脫鉤，改了也不會讓已派工的 Run 失效）；區分 `environment_blocked` 等 verdict（屬票 45 的 Verification Profile 範圍）。
- 影響：契約 JSON 多一個欄位（舊資料沒有這欄時視為空陣列）。暫時性的環境問題（例如工具一時不在 PATH）會讓 Task 回到 ready，需要使用者重新派工；細分判定留給票 45～50。
- 審閱：未審

### AI-149：驗證命令以使用者身分在工作目錄執行；契約目前只能由桌面憑證寫入
- 重要度：高
- 時間／階段：2026-10-22，票 13 實作
- 決策：新增 `CommandRunner` Port。正式版 `LocalCommandRunner` 以子程序直接執行（`CREATE_NO_WINDOW`、stdin 為 NUL、stdout／stderr 收集後保留結尾 16 KiB，證據再取結尾 4000 字元），逾時就丟棄等待並以 `kill_on_drop` 終止直接啟動的程序；**不放進 Job Object**，命令自己再開的子孫程序可能殘留。檔案檢查以 `WorkspaceFs::read_workspace_file` 讀取，只擋路徑字面上的逃逸，不處理工作目錄內指向外部的符號連結或 junction。命令輸出以 UTF-8 解讀（非 UTF-8 主控台輸出會變成替代字元）。
- 理由：契約目前只能經桌面管理 API 建立與修改（Agent 憑證被 guard 擋下），所以「要執行什麼命令」由使用者決定，與使用者在終端機自己執行相同；完整的程序樹控管與路徑沙箱超出本票範圍。
- 替代方案：把驗證命令放進 Job Object（需要把 `WindowsProcessControl` 的一次性執行版本抽出來，工作量大）；要求宣告命令檢查的契約必須有 `run_build_test` 能力（不同 Workspace 授權語意，而且驗證命令是 Runtime 自己執行、不是 Worker）。
- 影響：**Controller 的 `update_task`／`propose_task` 工具（票 17）若能寫入 `verification_checks`，等於讓 Agent 決定 Runtime 以使用者身分執行的命令**；屆時必須經過裁定或授權（D-15「修改完成條件或驗收標準」本來就必須送裁定）。
- 審閱：未審

### AI-150：成果接受交易的三項校驗與「受驗 Run」的定義
- 重要度：高
- 時間／階段：2026-10-22，票 13 實作
- 決策：受驗 Run = 該 Task 世代最大的 Run，而且它已 `succeeded`；沒有這樣的 Run 就沒有可接受的對象。接受基礎（basis）= 受驗 Run 的識別與世代、Task 目前的契約版本、成果 manifest（每個成果最新版本的 `artifact_id`＋版本）。自動檢查與手動接受都經同一個 Domain 判定（`verification::decide`）在單一寫入交易中校驗：Run 或世代不符 → 409 `acceptance_run_stale`；契約版本與檢視時不同，或不等於 Run 派工時的版本 → 409 `acceptance_contract_stale`；manifest 不同 → 409 `acceptance_manifest_stale`；拒絕時什麼都不寫。契約在 Run 派工後被改版時，Supervisor 不執行檢查，直接記一筆「未通過」（`note` 寫明版本），Task 回到 `ready`。資料庫另以唯一索引保證同一個 Run 最多一筆紀錄、同一個 Task 最多一筆「通過」。
- 理由：票面要求「成果接受交易同時校驗 Run generation、成果 manifest 與契約版本」與「過期 Run 的成果回報不能推進 Task」。過期 Run 的成果登記與結束事件本來就因不是寫入所有者而被拒絕（AI-89、AI-107），這裡再擋住「依過期觀察的接受」，包括 Supervisor 執行檢查期間使用者先手動接受的競態。依舊契約完成的工作在新契約下被接受，等於讓完成條件被悄悄改寫。
- 替代方案：manifest 只比對「成果是否存在」（無法發現檢視後被追加的新版本）；契約改版時讓 Task 停在 verifying 等使用者處理（沒有出口，只能取消）。
- 影響：手動接受的請求必須帶回檢視時的 `basis`（UI 原樣送回）。契約在 verifying 時被修改會讓這次成果作廢並需要重新派工。
- 審閱：未審

### AI-151：檢查未通過 Task 回到 ready，不自動重派；沒有「手動駁回」
- 重要度：中
- 時間／階段：2026-10-22，票 13 實作
- 決策：任何一項檢查未通過，Task 從 `verifying` 回到 `ready`（「可修正狀態」），證據（每項檢查的結果、輸出結尾、manifest）保存在驗證紀錄並顯示在工作面板。不自動放回派工佇列（沿用 AI-66、AI-96：沒有重試預算前不自動重派），也不計入任何預算（票 36）。重新派工時 Worker 的提示不含上次的失敗證據（Context Builder 屬票 39）。本票不提供「手動駁回」：沒有宣告檢查而成果不理想時，只能取消 Task 或修改契約。
- 理由：`ready` 是既有狀態表中唯一能再派工的狀態；`blocked` 需要裁定流程（票 22），`pending` 會要求重新准入但契約並沒有變。手動駁回不在驗收條件內。
- 替代方案：自動重新排隊（失敗會變成無限重派迴圈）；進入 `blocked` 並建立裁定（票 22／36 之後再評估）；提供駁回按鈕（多一條沒有要求的路徑）。
- 影響：失敗後要使用者再按一次派工；新的 Worker 不知道上次為什麼沒通過，直到票 39 把驗證紀錄放進 context。
- 審閱：未審

### AI-152：手動接受只記錄桌面憑證指紋；允許在自動檢查進行中手動接受
- 重要度：中
- 時間／階段：2026-10-22，票 13 實作
- 決策：`POST …/tasks/{task_id}/verification/accept` 屬管理 API，guard 只接受桌面憑證（Agent 憑證 403、無憑證 401）。接受者記為「桌面憑證雜湊的前 16 個十六進位字元」，寫進驗證紀錄與事件 `task_verification_recorded` 的 `accepted_by`，不記錄憑證本身。宣告了檢查的 Task 也可以手動接受（使用者優先）；與 Supervisor 的檢查結果競態時，先提交者生效，後者因 Task 已不是 verifying 而被拒絕，只留日誌。附註最多 1000 字元。
- 理由：spec「只有經桌面管理憑證提交者才是人工正式裁定」；桌面憑證每次啟動輪替，指紋足以對應到哪一次 Runtime 啟動簽發的憑證，又不洩漏可用的憑證。
- 替代方案：自動檢查進行中禁止手動接受（使用者要等最多 10 分鐘）；記錄完整憑證雜湊（沒有額外價值）。
- 影響：目前只有一個桌面使用者，`accepted_by` 只能區分「哪一次啟動的桌面 App」；多使用者不在範圍內。
- 審閱：未審

### AI-153：驗證是 Supervisor 的最後一步；背景迴圈另開工作執行；崩潰後重新執行檢查
- 重要度：中
- 時間／階段：2026-10-22，票 13 實作
- 決策：Supervisor 新增「驗證」步驟（在排程之後），找出 `verifying`、有受驗 Run、契約宣告了檢查（或契約已改版）的 Task，在交易外逐項取得事實，再以成果接受交易提交。背景迴圈把這一步放在另一個 tokio 工作執行（`verify_lock` 保證同時只有一輪），長時間的檢查不擋住啟動、取消與 lease 核對；`Supervisor::tick`（harness 的 `supervise()`）則等它做完，測試保持決定性。檢查途中 Runtime 崩潰時沒有任何紀錄，重啟後**重新執行全部檢查**（命令若有副作用會被執行兩次）。驗證結果寫入失敗（儲存層錯誤）時下一輪也會重新執行。
- 理由：先核對、再恢復：沒有結論就不猜測通過或失敗；驗證命令本來就應該可重複執行（建置、測試）。
- 替代方案：檢查前先持久化「驗證中」意圖（需要另一個狀態與恢復規則，對可重複的檢查沒有價值）；在 tick 內同步執行（10 分鐘的測試會讓取消與 lease 核對延後 10 分鐘）。
- 影響：完全退出（票 25）需要一併停止進行中的驗證命令；目前 Runtime 停止時只靠 `kill_on_drop`。
- 審閱：未審

### AI-154：驗證紀錄的資料模型與 API 形狀（migration 16）
- 重要度：低
- 時間／階段：2026-10-22，票 13 實作
- 決策：migration 16 `task_verification`（排在票 08 的 `shared_settings_version`、票 27 的 `quota`、票 18 的 `message_and_delivery`、票 22 的 `decision_request_and_decision` 之後）：識別、Workspace、Task、受驗 Run（唯一）、世代、契約版本、方式（`checks`／`manual`）、結果（`passed`／`failed`）、`evidence`（JSON：每項檢查結果、manifest、接受者、附註）、時間；觸發器禁止更新與刪除；部分唯一索引限制每個 Task 最多一筆 `passed`。端點：`GET …/tasks/{task_id}/verification`（`VerificationView`：basis、宣告的檢查、manifest、`automatic`、歷次紀錄）、`POST …/verification/accept`（`{ basis, note? }`）。工作面板 `GET …/tasks` 多回傳 `verifications`。Tauri 指令 `task_verification`、`task_accept`。驗證紀錄識別為 `ver_` 加 96 位元隨機值。
- 理由：沿用 AI-74、AI-105 的追加式表與路徑慣例；把證據放在 JSON 欄位，避免為每項檢查另開一張表。
- 替代方案：每項檢查一列（查詢單項較方便，但目前沒有這種查詢）。
- 影響：本票原本是第 12 個 migration，整合 rebuild/v1（票 08、27）時順延為第 14 個，再整合票 18 時順延為第 15 個（見 AI-156），再整合票 22 時順延為第 16 個（見 AI-158）；以本票分支（合併前）建立過的開發資料庫需刪除重建。
- 審閱：未審

### AI-155：契約編輯器以文字行宣告檢查，路徑與名稱不能含空白
- 重要度：低
- 時間／階段：2026-10-22，票 13 實作
- 決策：Web UI 契約編輯器的「確定性檢查」每行一項：`command: 程式 引數…`（預期結束碼 0）、`command[n]: …`（預期結束碼 n，可為負數）、`file: 相對路徑 [sha256]`、`artifact: 成果名稱 [sha256]`；引數以空白分隔、不支援引號，所以路徑、成果名稱與引數都不能含空白；大寫雜湊自動轉小寫；解析錯誤在送出時顯示且不送出；路徑是否在工作目錄內由 Runtime 判定。驗證失敗時 UI 只重新讀取驗證資料、不區分 409 的種類。
- 理由：沿用契約引用以文字行輸入的做法（AI-65），不必為三種檢查各做一套表單。
- 替代方案：結構化表單（含空白的路徑也能表達，但畫面複雜很多）。
- 影響：需要含空白的路徑時只能直接呼叫 API。
- 審閱：未審

### AI-156：票 13 整合 rebuild/v1（票 18）：migration 第 15 版、AI 編號順延
- 重要度：中
- 時間／階段：2026-10-22，票 13 修正（R054-13-fix）
- 決策：rebuild/v1 已合併票 18（`message_and_delivery`，第 14 版），本票的 `task_verification` 順延為第 15 個 migration，排在 `message_and_delivery` 之後；本票的決策由 AI-128～AI-135 順延為 AI-138～AI-145（整合分支已用到 AI-137；R054 再整合票 22 時順延為 AI-148～AI-155，本條為 AI-156，見 AI-158），移到 AI-137 之後，內容不變，只把 AI-144（現為 AI-154）的 migration 編號改為 15 並在影響加註這次順延。`docs/development.md` 驗證紀錄段落的 migration 編號同步改為 15。其他檔案（API 路由、Domain 模組、GLOSSARY）由 git 自動合併，兩邊新增的內容都保留。
- 理由：整合分支的條目與 migration 編號不能變動；本票尚未發布，順延不影響任何既有資料庫。驗證紀錄表只參照 `workspace`、`task`、`run`，與訊息表互不相依，先後順序不影響 schema。
- 替代方案：把本票的 migration 插在 `message_and_delivery` 之前（會改動整合分支已存在的版本號，已套用的開發資料庫會錯位）。
- 影響：以本票分支（合併前）建立過的開發資料庫需刪除重建。
- 審閱：未審

### AI-157：封存的 Workspace 中，verifying 的 Task 不再自動驗證
- 重要度：中
- 時間／階段：2026-10-22，票 13 修正（R054-13-fix）
- 決策：`verification::pending_check` 在 Workspace 已封存（或不存在）時回傳 `None`，所以 Supervisor 的驗證步驟不再為它執行確定性檢查，`VerificationView.automatic` 也是 `false`。封存 API 仍不檢查 Task 狀態；這種 Task 會停在 `verifying`，因為成果接受交易（自動與手動）都以 `workspace_archived` 拒絕。
- 理由：驗證 R051 發現封存後 `decide()` 一定拒絕提交，但待驗證清單沒排除封存的 Workspace，背景迴圈每秒重跑一次契約宣告的命令（每次最長 10 分鐘、可能有副作用）。在判定「是否待檢查」的同一個 Domain 函式排除，面板顯示與 Supervisor 行為就一致。
- 替代方案：封存 API 拒絕有 verifying Task 的 Workspace（改變 Workspace 封存的既有語意，超出本票）；封存時把 verifying 的 Task 改回 ready 或取消（多一條狀態轉移與事件，沒有要求）；允許封存後仍接受成果（與「封存後不再變更」的既有規則衝突）。
- 影響：若之後提供「取消封存」，這些 Task 會重新成為待驗證項目並照常執行檢查。
- 審閱：未審

### AI-158：票 13 整合 rebuild/v1（票 22）：migration 第 16 版、AI 編號再順延
- 重要度：中
- 時間／階段：2026-10-22，票 13 修正（R054-13-fix）
- 決策：R054 修正期間 rebuild/v1 合併了票 22（`decision_request_and_decision`，第 15 版，AI-138～AI-147），本票的 `task_verification` 順延為第 16 個 migration；本票的條目由 AI-138～AI-147 順延為 AI-148～AI-157，移到整合分支的 AI-147 之後，內容不變，只更新 AI-154 的 migration 編號與 AI-154、AI-156 中互相引用的編號。衝突合併方式：API 路由、前端 `TaskClient`／`DecisionClient` 型別與 HTTP／Tauri 用戶端、Tauri 的 `baton_api` 匯入、`App.test.tsx` 的假用戶端都兩邊保留；`docs/development.md` 的 crate 表以整合分支的列為準，再加回本票的 `verification.rs`、`CommandRunner`、`task_verification`、「成果接受」，驗證紀錄段落的 migration 編號改為 16。
- 理由：整合分支的條目與 migration 編號不能變動；本票尚未發布。驗證紀錄表與裁定表都只參照 `workspace`、`task`、`run`，互不相依，順序不影響 schema。
- 替代方案：把本票的 migration 插在裁定表之前（會改動整合分支已存在的版本號）。
- 影響：以本票分支（合併前）建立過的開發資料庫需刪除重建。
- 審閱：未審

### AI-159：知識條目與教訓共用一張 `knowledge_entry` 表；內容版本追加式、條目不可刪除
- 重要度：中
- 時間／階段：2026-10-22，票 37 實作
- 決策：票面的「knowledge_entries 與 lessons」實作為 migration 14 `knowledge` 的兩張表：`knowledge_entry`（決策 `decision`、教訓 `lesson`、一般知識 `fact` 以 `kind` 區分；保存範圍、可信狀態、來源、有效期間、內容版本號、聚合版本 `revision`、取代關係、撤銷／淘汰原因與目前內容的檢索文字 `search_text`）與追加式的 `knowledge_entry_version`（每個內容版本一列，觸發器禁止更新與刪除）。教訓特有的「觸發條件、根因或待驗假說、解法」放在內容 JSON 的 `lesson` 欄（只有教訓有）。`knowledge_entry` 有觸發器禁止刪除。變更歷史取自既有的追加式 `event` 表（`aggregate_type = 'knowledge_entry'`）。
- 理由：GLOSSARY 已定義「Decision 與 Lesson 都是 Knowledge Entry 的特定形態」；狀態、範圍、有效期間、版本、取代關係與搜尋對三種形態完全相同，拆兩張表只會複製一整套程式與索引。仿照 Task 契約（`task` + 追加式 `task_contract_version`）的做法，歷史不需要另一張稽核表。
- 替代方案：`knowledge_entry` 與 `lesson` 兩張表（教訓欄位成為獨立欄位，Guard 查詢較直接，但版本、狀態、搜尋要做兩份）；每個版本一列、以 lineage 串起（查目前狀態要取最大版本，狀態變更也要複製內容）。
- 影響：票 38（Guard）以條目 id 參照教訓；若 Guard 需要結構化的教訓欄位，可在 `lesson` JSON 內擴充或另加欄位。整合 rebuild/v1 時 migration 由第 12 個順延為第 14 個（見 AI-165），第二次整合再順延為第 16 個（見 AI-166），第三次整合再順延為第 17 個（見 AI-167）。
- 審閱：未審

### AI-160：驗收條件中的「決策」以知識分頁的決策條目滿足；裁定中心的正式 Decision 尚未納入搜尋
- 重要度：中
- 時間／階段：2026-10-22，票 37 實作
- 決策：「繁體中文兩字以上關鍵字可搜到決策與教訓」的「決策」，指 `kind = decision` 的知識條目（使用者在知識分頁記錄的已定案判斷與理由，可指定取代舊決策）。票 22／35 的裁定（Decision Request 經人工裁定產生的正式 Decision）目前不在本分支，所以沒有被寫入知識索引。
- 理由：本票只依賴票 12，票 22 仍在平行實作中，無法引用它的資料表；知識條目本身已具備 Decision 詞條要求的來源、理由、範圍、有效期間與取代關係。
- 替代方案：等待票 22 合併後再做（阻塞本票）；在本票預先建立裁定的資料表（與票 22 衝突）。
- 影響：票 35（完整裁定中心）或之後的整合，應在保存正式 Decision 的同一個交易中建立對應的 `kind = decision` 知識條目（來源指向該 Decision），才能用同一個搜尋找到裁定；在那之前，裁定中心的 Decision 搜不到。
- 審閱：未審

### AI-161：`record_lesson_candidate` 暫時開放給所有 Agent 憑證；不論誰提交都只能是候選教訓
- 重要度：高
- 時間／階段：2026-10-22，票 37 實作
- 決策：spec D-05 把 `record_lesson_candidate` 列為 Controller 專屬工具，但目前 Agent 憑證只有一種角色（Worker Run，見 AI-88；Controller 憑證屬票 16、17），所以本票讓任何 Agent 憑證都能呼叫。限制一律由 Runtime 決定、請求無法改變：(1) 只會建立 `kind = lesson`、`status = candidate` 的條目（請求帶 `status`、`kind` 會被忽略；Domain 的 `knowledge::create` 依來源決定狀態，Agent 來源只能是候選教訓）；(2) 範圍只能是憑證所屬 Run 的 Task（預設）、它的 Goal 或它的 Workspace，以 `scope` 字串指定、不接受任何識別，全域被拒絕；(3) Run 必須仍是寫入所有者，否則 409 `run_report_stale`、不寫入；(4) Agent 憑證不能呼叫知識分頁的管理 API（確認、修正、撤銷、建立都需要桌面憑證）。`search_knowledge` 同樣需要 Run 仍是寫入所有者（否則 401 `credential_revoked`），只回傳這個 Run 適用範圍內有效的條目。
- 理由：D-28 的核心是「候選須使用者確認才升為已驗證」，這由狀態規則保證，與提交者是 Controller 或 Worker 無關；讓 Worker 也能提交候選不會讓任何教訓繞過確認，而等 Controller 憑證出現才做工具會讓本票無法以真實 API 驗證。
- 替代方案：本票不提供 Agent 工具，只做 Domain 與管理 API（無法驗證「Controller 只能提交候選」）；以新的憑證角色模擬 Controller（會搶先決定票 16、17 的角色模型）。
- 影響：票 16／17 建立 Controller 憑證角色時，應把 `record_lesson_candidate` 限制為 Controller 角色（Worker 呼叫改回 403），或決定保留 Worker 提交候選的能力並更新本條目。在那之前 Worker 可以提交候選教訓，知識分頁的候選區會看到來源 Run。
- 審閱：未審

### AI-162：可信狀態與轉換規則、有效的定義、取代關係與並行修改校驗
- 重要度：中
- 時間／階段：2026-10-22，票 37 實作
- 決策：可信狀態五種：候選 `candidate`、已驗證 `verified`、已撤銷 `revoked`、已淘汰 `retired`、已被取代 `superseded`。狀態由來源決定：使用者在知識分頁建立的直接是已驗證，Agent 提交的是候選。轉換：確認（候選→已驗證）；修正（候選或已驗證，追加新內容版本、狀態不變，只改有效期限時不追加版本）；撤銷（候選或已驗證→已撤銷，表示內容是錯的，需原因；拒絕候選也用撤銷）；淘汰（只限已驗證→已淘汰，表示曾經正確但已不適用，需原因）；取代（使用者建立新條目時指定 `supersedes`，被取代者必須是同種類且已驗證，同一交易轉為已被取代並互相連結）。撤銷、淘汰、被取代都是終態。**有效 = 已驗證且現在在有效期間內**（`valid_from ≤ now < valid_until`），只有有效條目會被 Agent 取得或出現在預設搜尋結果。每個操作都必須帶 `expected_revision`，不符回 409 `knowledge_revision_conflict`。
- 理由：票面要求「可修正、撤銷、淘汰並保留歷史」；把「錯的」與「過時的」分開，事後追查教訓重犯率（spec #93）時才分得出是教訓錯了還是環境變了。expected_revision 沿用裁定的校驗方式（issue §6），避免兩個畫面互相覆蓋。
- 替代方案：確認時允許同時編輯內容（先修正再確認即可達成）；淘汰也允許用於候選（與撤銷重疊）；修正後回到候選重新確認（使用者自己修正的內容不需要再確認自己）。
- 影響：過期的已驗證條目仍是「已驗證」，只是不有效；延長期限需透過修正。Agent 提交的候選若被使用者修正，仍保留 Agent 來源。
- 審閱：未審

### AI-163：全文檢索的實作：每次啟動重建 FTS5 索引；2 字元關鍵字走 LIKE；降級的偵測與顯示
- 重要度：中
- 時間／階段：2026-10-22，票 37 實作
- 決策：(1) FTS5 虛擬表 `knowledge_fts(entry_id UNINDEXED, content, tokenize = 'trigram')` 不放在 migration，而是 Store 每次開啟時建立（不存在時）並從 `knowledge_entry.search_text` 整個重建；建立或重建失敗（FTS5 或 trigram 不可用、schema 較新）就以 LIKE 模式運作。之後的寫入在同一個交易中更新索引。(2) 查詢以空白分詞、全部都要出現（AND）；每個詞至少 2 個字元（單字元回 400 `knowledge_query_invalid`），最多 8 個詞、200 字元。FTS5 可用時，3 個字元以上的詞以片語 MATCH 查 trigram 索引；**2 個字元的詞一律以 LIKE 比對 `search_text`**（trigram 索引無法涵蓋少於 3 字元的詞）；FTS5 不可用時全部以 LIKE（`%`、`_`、`\` 已跳脫）。(3) 結果依最後更新時間由新到舊，最多 100 筆，不做相關度排序。(4) 降級顯示：列表與 `search_knowledge` 的回應帶 `search_mode`（`full_text`／`like_fallback`），Runtime 啟動時在日誌記一行降級訊息，知識分頁顯示降級提示。(5) 測試以 `RuntimeOptions.full_text_search = false`（Store 的 `StoreOptions`、harness 的 `without_full_text_search`／`set_full_text_search`）重現「FTS5 不可用」，因為 bundled SQLite 一定有 FTS5。
- 理由：每次重建讓索引永遠是可丟棄的衍生資料：降級期間寫入的條目、備份還原或手動修改資料庫之後都不會漏，條目數量在個人使用規模下重建成本可忽略。中文常見的兩字詞（「快取」「部署」）必須搜得到，trigram 對它們無能為力，LIKE 結果相同只是需要逐筆比對。
- 替代方案：FTS5 表放進 migration（FTS5 不可用時 migration 失敗、Runtime 無法啟動，違反「退回 LIKE」）；外部內容表（external content）＋觸發器維護（觸發器依賴 FTS5 存在，降級時寫入會失敗）；以 bigram 自訂 tokenizer 支援兩字詞（需要載入擴充，bundled 不提供）；bm25 排序（LIKE 模式無法對應，兩種模式結果順序會不同）。
- 影響：條目數量很大時，啟動重建與 2 字元的 LIKE 掃描會變慢，需要時再改為增量或加上 bigram 索引。搜尋沒有相關度排序。
- 審閱：未審

### AI-164：知識 API 與 Agent 工具的形狀與限制
- 重要度：低
- 時間／階段：2026-10-22，票 37 實作
- 決策：管理 API（桌面憑證）：`GET /api/v1/knowledge?workspace_id=&q=&include_inactive=`（省略 `workspace_id` 時只列全域；`include_inactive=true` 時連同無效條目一起回傳，供「顯示歷史」與候選區使用）、`POST /api/v1/knowledge`、`GET /api/v1/knowledge/{id}`（目前狀態、全部內容版本、變更歷史）、`POST …/{id}/confirm|correct|revoke|retire`。範圍由 Runtime 核對存在與一致性，Task 範圍的 Goal 一律取自 Task。Agent 工具 `record_lesson_candidate`、`search_knowledge` 放在 `/agent/v1/tools/` 之下（AI-88）。Agent 看得到的範圍：全域、它的 Workspace、它的 Goal、它的 Task；知識分頁看得到：全域與該 Workspace 內所有範圍。限制：標題 200、內文 10,000、教訓各欄 2,000、標籤 20 個各 50、證據 20 項各 1,000、原因與修正說明 1,000 字元。
- 理由：沿用既有管理 API 與 Agent API 的分流與錯誤代碼慣例；數值是預設值，改起來便宜。
- 替代方案：知識 API 掛在 `/api/v1/workspaces/{id}/knowledge` 下（全域條目沒有自然的位置）；搜尋改用 POST 本文（查詢字串已足夠，中文由用戶端 URL 編碼）。
- 影響：知識分頁目前沒有 Goal／Task 範圍的建立入口（API 支援），也不能在 UI 修改有效期間（修正時沿用原值）；需要時由後續票補上。Context Builder（票 39）載入教訓時應沿用 `search_knowledge` 的「適用範圍 + 有效」判定。
- 審閱：未審

### AI-165：票 37 整合 rebuild/v1（票 08、27）：migration 與 AI 編號順延、錯誤代碼輔助函式合併
- 重要度：低
- 時間／階段：2026-10-22，票 37 實作（結束前合併 rebuild/v1）
- 決策：票 08（`shared_settings_version`，第 12 個）與票 27（`quota`，第 13 個）已先合併，本票的 migration `knowledge` 由第 12 個順延為第 14 個；兩者互不參照（`knowledge_entry` 只參照 `workspace`、`goal`、`task` 與自己），先後不影響 schema。本票的 AI 條目由 AI-110～AI-115 順延為 AI-128～AI-133（第二次整合再順延為 AI-148～AI-153，見 AI-166；第三次整合再順延為 AI-159～AI-164，見 AI-167），內容不變（程式碼與文件沒有引用這些編號）。Web UI 原本新增的 `hasErrorCode` 與票 08 的 `errorCode` 功能重疊，改用整合分支的 `errorCode`，刪除 `hasErrorCode`。其餘衝突（API 路由與匯出、Tauri 指令清單、`AppClient`、開發指南的目錄表與情境測試清單）一律兩邊都保留。
- 理由：整合分支已有的 migration 與 AI 編號不可變動（AI-108 同樣做法）；兩個做同一件事的輔助函式只留一個。
- 替代方案：保留兩個輔助函式（重複且判斷規則不同：`hasErrorCode` 以子字串比對，`errorCode` 只認訊息開頭的代碼格式）。
- 影響：以本票分支（合併前）建立過的開發資料庫，其 `schema_migrations` 第 12 筆是 `knowledge`，與整合後不一致，需刪除重建；尚無發布版本，不提供轉換。
- 審閱：未審

### AI-166：票 37 整合：第二次整合 rebuild/v1（票 18、22）：migration 第 16 版、AI 編號再順延
- 重要度：中
- 時間／階段：2026-10-22，票 37 整合（R056-37-integrate）
- 決策：rebuild/v1 已合併票 18（`message_and_delivery`，第 14 個 migration）與票 22（`decision_request_and_decision`，第 15 個），本票的 `knowledge` 順延為第 16 個 migration，排在兩者之後；開發指南的「migration 14 `knowledge`」同步改為 16。整合分支的 AI 編號已用到 AI-147（AI-128～AI-134 為票 18，AI-137 為主控，AI-138～AI-147 為票 22），本票的決策由 AI-128～AI-134 順延為 AI-148～AI-154，移到 AI-147 之後，內容不變，只把條目間的互相引用改成新編號，並在 AI-148、AI-154 的歷史說明加註再次順延；票 16 票面引用的「票 37 的 AI-130」改為 AI-150。（本 run 先合併票 18 時曾暫編為 AI-138～AI-145、migration 第 15 個；合併過程中 rebuild/v1 又合併了票 22，所以再順延一次，中間的 merge commit 保留在歷史中。）衝突合併方式：Agent 工具變成 11 個（原有 4 個＋票 22 的 `request_decision`、`permission_prompt`＋票 18 的 `send_message`、`read_inbox`＋`register_artifact`＋本票的 `record_lesson_candidate`、`search_knowledge`），順序依 shim 工具定義，shim 的已知工具清單、情境測試的名稱斷言與 API 路由兩邊保留；shim 情境測試改名為 `tools_list_has_the_eleven_tools_with_their_required_fields`，stdio 測試的工具數改為 11，各票的 schema 斷言都保留。Domain、Store、API、`baton-client`、Tauri 的匯入與指令兩邊保留；Web 的 `Opened.view` 同時有 `decisions` 與 `knowledge`，側欄依序顯示「裁定」「成果與驗證」「知識」按鈕；開發指南的目錄表與情境測試清單以詞為單位合併，兩邊新增的項目都保留。
- 理由：整合分支的條目與 migration 編號不能變動；本票尚未發布，順延不影響任何既有資料庫。知識表只參照 `workspace`、`goal`、`task` 與自己，與訊息、裁定的資料表互不相依，先後不影響 schema。
- 替代方案：本票的 AI 編號用整合分支空著的 AI-135、AI-136（其他平行票的分支已占用這兩個編號，之後整合還會再撞號）。
- 影響：以本票分支（先前任一次整合後）建立過的開發資料庫，`schema_migrations` 中 `knowledge` 的版本號與整合後不一致，需刪除重建。之後再新增 Agent 工具，要同時更新 shim 的兩個工具數量測試。第三次整合後本票條目再順延為 AI-159～AI-166、migration 為第 17 個（見 AI-167），本條目內的編號是當時的編號。
- 審閱：未審

### AI-167：票 37 整合：第三次整合 rebuild/v1（票 13）：migration 第 17 版、AI 編號再順延
- 重要度：中
- 時間／階段：2026-10-22，票 37 整合（R061-37-integrate）
- 決策：rebuild/v1 已合併票 13（`task_verification`，第 16 個 migration，AI-148～AI-158），本票的 `knowledge` 順延為第 17 個 migration，排在其後；Store 的 migration 註解與開發指南的「migration 16 `knowledge`」同步改為 17。本票的條目由 AI-148～AI-155 順延為 AI-159～AI-166，移到整合分支的 AI-158 之後，內容不變，只把條目間互相引用的編號改成新編號，並在 AI-159（原 AI-148）、AI-165（原 AI-154）、AI-166（原 AI-155）的歷史說明加註再次順延；票 16 票面引用的「票 37 的 AI-150」改為 AI-161。自動合併時原 AI-155 的 `- 審閱：未審` 行與票 13 的 AI-148 標題相鄰而被吃掉，重建檔案時補回。衝突合併方式：API 的模組說明兩邊端點都列出；Store 的 migration 先放票 13 再放本票；Tauri 的 `baton_api` 匯入兩邊保留；`runtimeClient.test.ts` 的知識測試與驗證測試都保留；開發指南的 crate 表以整合分支的列為準，再加回本票的 `knowledge.rs`、`knowledge_entry`／`knowledge_fts`、`/api/v1/knowledge`、Agent 工具與 `RuntimeOptions.full_text_search`。
- 理由：整合分支的條目與 migration 編號不能變動；本票尚未發布。知識表只參照 `workspace`、`goal`、`task` 與自己，驗證紀錄表只參照 `workspace`、`task`、`run`，互不相依，先後不影響 schema。
- 替代方案：把本票的 migration 插在驗證紀錄表之前（會改動整合分支已存在的版本號）。
- 影響：以本票分支（先前任一次整合後）建立過的開發資料庫，`schema_migrations` 中 `knowledge` 的版本號與整合後不一致，需刪除重建。
- 審閱：未審

### AI-168：完全退出停下的 Run 以 interrupted 結束，Task 退回 ready 並自動重新排隊；停不下來的保留寫入所有權
- 重要度：高
- 時間／階段：2026-10-22，票 25 實作
- 決策：程序確認停止（`StoppedGracefully`／`ForceKilled`）或根本沒有程序（`queued`、啟動中但以 command_id 找不到）的 Run，以 `interrupted`（`end_reason = stopped_by_full_exit`）結束並釋放寫入所有權；Task 只在仍是 `running` 時退回 `ready`（`blocked`、`verifying` 等不動），並在**同一個交易**中重新放進派工佇列（優先序 0，原本的優先序已不存在）。使用者先前已要求取消的 Run 則完成取消（Task 不重新排隊）。`StillAlive`、停止呼叫失敗、到總上限仍沒有結果的記為停止異常，Run 維持 `running` 並保留 lease。重新開啟核對時，這些 Run 的程序若確定已不在，以 `process_gone_after_full_exit` 中斷、Task 回 `ready` 但**不**自動重新排隊；仍存活的維持原狀（不會再啟動第二個 Worker）。
- 理由：票面要求「完全退出不會把 Task 標為取消或完成」與「重開後 A 的合格工作自動恢復」，被退出停下的工作必須有恢復的入口；放回佇列後，「續跑」就只是解除暫停，由排程照常判定授權、相依、帳號與容量（即「合格」）。這和 AI-96「Run 失敗不自動重新排隊」不衝突：退出不是失敗，而是使用者明確要求的暫停。停不下來的程序若釋放 lease，續跑時會產生第二個寫入者（使用者故事 43）。查證後才知道已不在的程序，結果不明（它可能已完成部分工作），依 AI-67 的原則交給使用者重新派工。
- 替代方案：退出時把 Run 標為 `cancelled`（語意上是使用者取消，且 AI-66 會讓人誤以為 Task 被放棄）；不重新排隊，續跑時再掃描「被退出停下的 Task」（需要另外記錄哪些 Task 屬於這一類，且會和使用者的手動派工打架）；停止異常也釋放 lease（可能兩個寫入者）。
- 影響：被停下的 Task 在重新開啟前一直排隊、等待原因是 `runtime_stopping`；停止異常的 Task 在核對前保持 `running`。重新排隊不保留原本的優先序。
- 審閱：未審

### AI-169：停止意圖的保存方式與阻擋點：單列狀態加各 Workspace 暫停，擋在 Scheduler 與 launch intent
- 重要度：高
- 時間／階段：2026-10-22，票 25 實作
- 決策：停止意圖存在單列表 `runtime_lifecycle`（`phase` = `running`／`stopping`／`stopped`、`version`、最近一次停止報告 JSON），`stopping` 與 `stopped` 都算停止意圖生效，直到重新開啟。要求退出的同一個交易中，把每個使用中的 Workspace 標為暫停（`workspace_lifecycle.paused_since`）。阻擋點只有兩個：（1）Scheduler 的 `Capacity.halt`（`lifecycle::dispatch_halt`），`place` 一看到就回等待，所以佇列重試、額度恢復後的派工、API 的新派工要求與未來的交接（票 28 以 `SlotKind::Handoff` 呼叫同一個 `place`）都被擋；（2）寫入 launch intent（`BeginLaunch`）的交易再檢查一次（`RunError::LaunchHalted`），擋住停止意圖寫入前就已派出、還沒啟動的 Run。等待原因新增第七類 `lifecycle`（`runtime_stopping`、`awaiting_resume`）。上次退出後尚未重新開啟（`stopped`）時再按「完全退出」，視為新的退出要求（版本加一），停止中則冪等。
- 理由：ADR-0001 要求「排程、Outbox 與 supervisor 啟動路徑都必須檢查停止意圖，不能只停排程器」；把檢查放在所有派工都會經過的 `place` 與所有啟動都會經過的 `BeginLaunch`，新加入的路徑（交接、重試預算）不必各自記得檢查。暫停在退出時就寫入，重新開啟只需要決定「誰解除」，不必回頭推算哪些 Workspace 受影響。
- 替代方案：只在 Supervisor 背景迴圈檢查（API 派工、未來的交接會漏掉）；不記錄 Workspace 暫停、重新開啟時才逐一判定（重新開啟前新增的 Workspace 會被誤判，且「繼續工作」沒有可以清除的狀態）；把停止意圖放進 `RunContext`（要改動所有建構點，與排程等待原因的呈現脫節）。
- 影響：交接（票 28）與重試預算（票 36）只要經過 `scheduler::place`／`evaluate` 就自動遵守停止意圖；若另闢直接呼叫 `RunCommand::Dispatch` 的路徑，必須先呼叫 `lifecycle::dispatch_halt`。
- 審閱：未審

### AI-170：重新開啟的流程：先核對再解除；資料庫與生命週期檔取保守者；以版本防止清掉新的退出要求
- 重要度：中
- 時間／階段：2026-10-22，票 25 實作
- 決策：Runtime 啟動時讀資料庫與 `<資料目錄>\lifecycle.json`：只有檔案有停止意圖、或檔案存在卻讀不懂（毀損）都視為停止，並把停止意圖補記進資料庫（同時暫停每個使用中的 Workspace、記一筆說明）；只有資料庫有時把它鏡像回檔案。停止意圖仍生效時，Supervisor 的第一次 `tick` 先核對（處理啟動中的 Run：以 command_id 找回或中斷；對每個有程序身分的 Run 不論 lease 是否過期都 `inspect`，`Dead` 才中斷），再以「啟動時看到的生命週期版本」做條件式重新開啟：版本已改變（核對期間使用者又按了完全退出）就不重新開啟。重新開啟後把檔案改寫為沒有停止抑制。
- 理由：spec D-25「兩者不一致時採較保守者」；票面「重新開啟先核對，再依設定續跑」。核對放在 Supervisor 的同一把鎖裡，與一般處理不會交錯；版本比對避免「剛重新開啟的 Runtime 把使用者新的停止意圖清掉」。
- 替代方案：重新開啟在 `Runtime::start` 內同步完成（測試無法決定核對時機，且啟動會被慢的 `inspect` 卡住）；檔案毀損時忽略檔案（可能在使用者要求停止後被 launcher 自動啟動）。
- 影響：Runtime 啟動到第一次處理之間（正式執行約 1 秒），API 仍回報停止意圖生效、「繼續工作」回 409。重新開啟時寫檔失敗只記日誌：下次啟動時會再被視為停止（較保守），使用者需要再次重新開啟。
- 審閱：未審

### AI-171：「保存並停止」的通知方式：Agent 回報的回應帶 `stop_requested`，強制力仍靠程序樹終止
- 重要度：中
- 時間／階段：2026-10-22，票 25 實作
- 決策：停止意圖生效後，`report_progress`、`save_checkpoint`、`report_blocked` 的成功回應帶 `stop_requested: true`（平常是 `false`），Worker 據此保存 checkpoint 並結束。Runtime 不另外對程序送出任何訊號；停止流程對每個 Run 呼叫 `ProcessControl::stop(identity, 30 秒)`：寬限期內等程序自行結束，逾時以 Job Object 終止整棵程序樹（AI-72）。
- 理由：`claude -p` 執行中沒有可靠的輸入管道（stdin 在啟動時就導向 NUL，AI-72），送主控台中斷訊號需要與 Worker 共用主控台，且 Windows 上無法對特定程序樹可靠地送 Ctrl+C。回報的回應是 Worker 本來就會定期呼叫的管道，不需要新的工具或協定。
- 替代方案：新增 `should_stop` 工具讓 Worker 輪詢（Worker 要記得呼叫，提示也要改）；送 `CTRL_BREAK_EVENT`（需要 `CREATE_NEW_PROCESS_GROUP` 與主控台，Validation Required 的範圍更大）。
- 影響：Worker 在寬限期內沒有回報就收不到通知，只能被終止；Context Builder（票 39）與真實 Adapter（票 14）的 Worker 提示需要寫明「回應出現 `stop_requested` 時保存 checkpoint 並結束」。30 秒是否足夠屬 Validation Required。
- 審閱：未審

### AI-172：40 秒上限的實作與停止流程結束後的收尾
- 重要度：中
- 時間／階段：2026-10-22，票 25 實作
- 決策：全部 Run 的 `stop` 並行執行，寬限期 30 秒（ProcessControl 在之後 10 秒內終止並確認，AI-72 的既有行為），Runtime 另以 `寬限期 + 確認上限 = 40 秒` 的真實時間作為安全網：到時還沒回來的停止記為停止異常 `stop_unconfirmed`。停止期限（`deadline_at`）以 Clock 記在停止報告中。停止流程期間握住 Supervisor 的 tick 鎖。結束後 Runtime 再提供 API 5 秒（讓 App 讀到最終結果）才結束服務；測試 harness 不自行結束（`exit_linger: None`），以 `exit_timing` 縮短上限測試卡住的情境。
- 理由：spec D-11 的總上限是 40 秒；ProcessControl 的 `stop` 本身就有上限，但 Port 的實作可能卡住（例如 Windows API 沒有回應），Runtime 不能把有界性完全交給 Port。App 每 0.5 秒輪詢，5 秒足以讀到結果；Runtime 若永遠不結束，App 當掉時會留下無主的 Runtime。
- 替代方案：停止依序執行（N 個 Run 會變成 N × 40 秒）；Runtime 停止流程結束立即結束服務（App 讀不到停止異常）。
- 影響：從按下「完全退出」到 Runtime 結束最長約 45 秒；第 40 秒才回來的 `ForceKilled` 若剛好晚於安全網，會被記為 `stop_unconfirmed`（保守）。
- 審閱：未審

### AI-173：桌面殼：關窗隱藏到系統匣、完全退出由 Web UI 顯示進度，殼有 60 秒後備
- 重要度：中
- 時間／階段：2026-10-22，票 25 實作
- 決策：關閉主視窗改為隱藏（`prevent_close` + `hide`），App 留在系統匣；系統匣選單有停用的狀態項目（每 3 秒以 `GET /api/v1/lifecycle` 更新：運作中與進行中 Run 數、完全退出中、已停止、無法連線）、「開啟 Baton」（左鍵點圖示亦同）與「完全退出」。「完全退出」顯示主視窗並對 Web UI 發 `baton://full-exit-requested`；Web UI 呼叫 `lifecycle_exit`、每 0.5 秒輪詢並顯示每個 Run 的停止結果與期限倒數；沒有停止異常約 1 秒後自動 `app_quit`，有異常則列出並等使用者按「關閉 Baton」；輪詢時 Runtime 已結束也視為完成。要求退出本身失敗時顯示原因與「重試」「返回」，不自動結束 App。殼在按下後 60 秒若 App 仍未結束，自行送出完全退出（冪等）再結束 App。HTTP 版用戶端（瀏覽器開發用）的 `app_quit` 與事件監聽是 no-op。
- 理由：票面要求「系統匣提供狀態、重新開啟與完全退出」「顯示停止進度與異常」；有異常時自動關閉會讓使用者看不到「哪些程序可能還在背景」。後備避免 Web UI 卡住時 App 永遠不結束；在系統匣按「完全退出」本身就是停止意圖，所以後備也送出退出要求。
- 替代方案：殼在 Rust 端自己輪詢並以原生對話框顯示進度（與 Web UI 的狀態呈現重複）；有異常也自動結束 App（異常只留在下次開啟才看得到）。
- 影響：沒有單一執行個體機制，App 隱藏在系統匣時再執行一次執行檔會開出第二個 App（兩者連同一個 Runtime，不會有第二個 Runtime 或 Worker）；可在後續票加入 `tauri-plugin-single-instance`。系統匣與關窗行為只有編譯與元件測試，Windows 上的實際操作屬 Validation Required（已寫入 `docs/development.md`，建議列入票 51）。
- 審閱：未審

### AI-174：生命週期 API、資料表與事件的形狀
- 重要度：低
- 時間／階段：2026-10-22，票 25 實作
- 決策：端點：`GET /api/v1/lifecycle`（階段、`stop_intent_active`、時間參數、進行中或最近一次的停止報告、進行中 Run 數、各使用中 Workspace 的 `auto_resume_after_exit`／`paused_since`／`awaiting_resume`）、`POST /api/v1/lifecycle/exit`（202）、`PUT /api/v1/workspaces/{id}/lifecycle`（`{ auto_resume_after_exit }`）、`POST /api/v1/workspaces/{id}/resume`（停止意圖生效時 409 `stop_intent_active`）。停止報告的每個 Run：`outcome` = `pending`／`not_started`／`stopped_gracefully`／`force_killed`／`anomaly`，異常再帶 `anomaly` = `still_alive`／`stop_failed`／`stop_unconfirmed` 與說明。migration 16：`runtime_lifecycle`（單列）與 `workspace_lifecycle`（沒有列的 Workspace 等同預設：自動續跑關閉、未暫停）。事件：`full_exit_requested`、`full_exit_completed`（含異常清單）、`runtime_reopened`（含續跑與等待的 Workspace）、`workspace_paused_by_exit`、`workspace_resumed`、`workspace_auto_resume_changed`；停止進度的逐筆更新不追加事件。UI：Workspace 清單每列有「完全退出後自動續跑」勾選與暫停中的「繼續工作」，等待原因第七類標籤為「完全退出」。
- 理由：一個總覽端點同時供系統匣、退出進度與 Workspace 清單使用；進度更新很頻繁，事件只記開始、結束與重新開啟。
- 替代方案：把 Workspace 設定放進 `Workspace` 本身（要改 Workspace 狀態機、API 與既有測試）；停止進度用 SSE（事件串流目前還沒有實際事件，輪詢已足夠）。
- 影響：migration 若與平行開發的票衝突，依 AI-75／AI-86 的方式順延（整合時已順延為第 18 個，見 AI-175、AI-176、AI-178、AI-179）。「跨登入自動續跑」（使用者故事 17 的另一個設定）屬票 33，屆時加在同一張 `workspace_lifecycle` 表。
- 審閱：未審

### AI-175：票 25 整合 rebuild/v1（票 12、08）：migration 與 AI 編號順延
- 重要度：中
- 時間／階段：2026-10-22，票 25 實作（合併 rebuild/v1）
- 決策：(1) 整合分支已有 `artifact`（第 11 版）與 `shared_settings_version`（第 12 版），本票的 `lifecycle` migration 順延為第 13 版；三者沒有共用資料表。(2) 本票原本的 AI-103～AI-109 與整合分支的票 12、08 條目重號，順延為 AI-121～AI-127（內文沒有互相引用，只把 AI-127 的 migration 編號改為 13）；整合分支的條目沒有改動。(3) `baton-api`、`baton-client`、Tauri 殼、Web UI 與 `docs/development.md` 兩邊新增的端點、指令、型別、樣式與段落全部保留（生命週期與成果、共同設定並存）。
- 理由：已發布的 migration 不可修改，順延尚未合併的本票版本（與 AI-86、AI-117 相同做法）。
- 替代方案：把本票的 migration 插在前面（會改動整合分支已有的版本號，不可行）。
- 影響：開發中的本機資料庫若套用過本票舊版本號的 migration，需要重建。
- 審閱：未審

### AI-176：票 25 再次整合 rebuild/v1（票 27）：migration 第 14 版、AI 編號再順延
- 重要度：中
- 時間／階段：2026-10-22，票 25 修正 run R049（合併 rebuild/v1）
- 決策：(1) 整合分支已先合併票 27，其 migration `quota` 是第 13 版，本票的 `lifecycle` 順延為第 14 版（排在 `quota` 之後；兩者沒有共用資料表）。(2) 本票條目原本的 AI-121～AI-128 與票 27 的 AI-121～AI-127 重號，順延為 AI-128～AI-135，移到票 27 條目之後；條目內的交互引用（AI-127 中的「見 AI-128」→ AI-134 中的「見 AI-135、AI-136」）與 migration 編號（AI-134 的 13 → 14、`docs/development.md`）同步更新，記錄歷史編號的敘述（AI-135）保留原文。原 AI-127、AI-128 缺少的審閱行與空行在順延時補上。(3) 其餘衝突兩邊都保留：Supervisor 同時有生命週期（重新開啟、完全退出）與額度輪詢、Adapter 額度事件；`baton-api`、`baton-client`、`baton-domain`、`baton-store` 的模組與匯出、Tauri 指令（`app_quit` 與 `profile_set_quota_limits`）、開發指南的 crate 表與情境測試清單合併為同時描述兩張票。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票（與 AI-117、AI-119、AI-175 同）。
- 替代方案：把兩個 migration 合併成一個（會改動整合分支已有的第 13 版）。
- 影響：已用本票分支建過的開發資料庫，`lifecycle` 記在第 13 版，與整合後的編號不一致，需要刪除重建。
- 審閱：未審

### AI-177：完全退出的 40 秒從要求退出時算起；生命週期檔在回應前寫入，等 Supervisor 的鎖也有上限
- 重要度：高
- 時間／階段：2026-10-22，票 25 修正 run R049（驗證 R047 未通過）
- 決策：修正 AI-172「停止流程期間握住 tick 鎖」造成的延遲：(1) `POST /api/v1/lifecycle/exit` 持久化資料庫後、回應 202 前就把停止意圖寫進生命週期檔（寫入失敗只記日誌，停止流程開始時再鏡像一次並把失敗記在停止報告的 notes）。(2) 停止流程的期限改由停止報告持久化的 `deadline_at` 減去 Clock 的目前時間換算成真實時間，等 tick 鎖的時間也算在內。(3) Supervisor 增加「完全退出進行中」的旗標；處理取消時呼叫 `ProcessControl::stop` 以 `select!` 同時等這個旗標，一看到就放棄等待並讓出 tick 鎖，該 Run 由停止流程再要求停止一次（同一個寬限期、共用總上限）。(4) 其他握鎖的處理卡住（例如 Port 的 spawn 不回來）時，等鎖以期限為上限；到期限仍拿不到鎖就**不**要求任何程序停止（不能在另一個處理進行中停止程序或結束 Run），把全部尚未結束的 Run 記為 `stop_unconfirmed`、保留寫入所有權，退出照常進入 `stopped`。
- 理由：驗收條件要求退出在 40 秒內結束；取消中的 `stop` 最長 30 + 10 秒且依序處理，Port 卡住時更永遠不結束，桌面殼的 60 秒後備會先關掉 App，使用者看不到停止異常。spec D-25 要求停止意圖先寫入資料庫與生命週期檔再開始停止，不應排在鎖後面。讓出鎖而不是不握鎖，是因為停止流程仍需要與 `launch_pending` 互斥：否則可能把啟動中的 Run 結束後，那個 spawn 才完成。
- 替代方案：處理取消時完全不握 tick 鎖、改在背景任務停止程序（現有情境測試依賴「一次 `supervise` 就確認取消」的確定性，改動面大）；停止流程完全不拿 tick 鎖（啟動中的 Run 會有競態）；只把等鎖加上逾時、不讓出（取消卡住時其他 Run 一個都沒被要求停止）。
- 影響：從按下「完全退出」到停止流程結束最長 40 秒（加上資料庫寫入），Runtime 結束仍再加 5 秒。被讓出的取消在停止流程中可能收到第二次 `stop`（Port 的 `stop` 必須可重複呼叫且可中途放棄；Windows 實作是輪詢 Job 內的程序數再 `TerminateJobObject`，放棄等待不留下半套狀態，重複呼叫沒有副作用）。Clock 被調整時，期限以持久化值為準（Clock 已過期限則不再等待，全部記為無法確認）。
- 審閱：未審

### AI-178：票 25 第三次整合 rebuild/v1（票 18、22）：migration 第 16 版、AI 編號再順延
- 重要度：中
- 時間／階段：2026-10-22，票 25 修正 run R049（合併 rebuild/v1）
- 決策：(1) 整合分支在本次修正期間先後併入票 18（`message_and_delivery`，migration 14）與票 22（`decision_request_and_decision`，migration 15），本票的 `lifecycle` 順延為第 16 版，排在最後；彼此沒有共用資料表。(2) 整合分支已用到 AI-147（AI-135、AI-136 空號，依規則不回填），本票條目原本的 AI-128～AI-137 順延為 AI-148～AI-157，移到票 22 條目之後；條目內的交互引用（AI-154 的「見 AI-155、AI-156、AI-158」、AI-156 的「與 AI-155 同」、AI-157 的「修正 AI-152」）與 migration 編號（AI-154、`docs/development.md` 改為 16）同步更新，記錄歷史編號的敘述（AI-155、AI-156 的決策內文）保留原文。(3) 其餘衝突兩邊都保留：`baton-api`／`baton-domain`／`baton-store` 的模組與匯出、`RuntimeClient` 的生命週期與裁定方法、Tauri 的型別匯入；Web UI 的 `AppClient`（HTTP 與 Tauri 兩種實作）同時有生命週期與裁定方法，`App` 同時掛上 `ExitProgress` 與 `DecisionCenter`，`WorkspaceSidebar` 採票 22 的參數（含裁定入口）並保留自動續跑說明，`styles.css` 兩邊新增的區塊依序並存，元件測試兩邊都保留；開發指南的 crate 表與情境測試清單合併為同時描述三張票。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票（與 AI-117、AI-119、AI-175、AI-176 同）。
- 替代方案：把本票條目放進 AI-135、AI-136 的空號（會與條目順序脫節，且其他平行的票也可能想用）。
- 影響：已用本票分支建過的開發資料庫，`lifecycle` 記在第 13 或 14 版，與整合後的編號不一致，需要刪除重建。
- 審閱：未審

### AI-179：票 25 第四次整合 rebuild/v1（票 13、37）：migration 第 18 版、AI 編號再順延、Supervisor 同時有生命週期與驗證
- 重要度：中
- 時間／階段：2026-10-22，票 25 整合 run R063-25-integrate
- 決策：(1) 整合分支先後併入票 13（`task_verification`，migration 16）與票 37（`knowledge`，migration 17），本票的 `lifecycle` 順延為第 18 版，排在最後；三者沒有共用資料表，都會套用。整合途中 rebuild/v1 又併入票 37，所以分兩次合併：第一次（只含票 13）的合併 commit 暫時把本票編成 migration 17、AI-159～AI-170，第二次合併後才定為下面的編號。(2) 整合分支已用到 AI-167（票 13 的 AI-148～AI-158、票 37 的 AI-159～AI-167），本票條目原本的 AI-148～AI-158 順延為 AI-168～AI-178，移到票 37 條目之後；條目內的交互引用（AI-174 的「見 AI-175、AI-176、AI-178、AI-179」與 migration 編號 18、AI-176 的「與 AI-175 同」、AI-177 的「修正 AI-172」、AI-178 理由中的「AI-175、AI-176」）與 `docs/development.md` 的 migration 編號同步更新；記錄歷史編號的敘述（AI-175、AI-176、AI-178 的決策內文）保留原文。程式碼與票面沒有引用這些編號。(3) 其餘衝突兩邊都保留：`Supervisor::new` 同時接收成果根目錄（票 13）與 `LifecycleSettings`（本票）；`supervise_once` 先做重新開啟再做原本的六個步驟（註解改為「七個步驟」），驗證仍是最後一步（開發指南改為「第 8 步」）；`RuntimeOptions` 與測試 harness 同時有 `exit_timing`／`exit_linger`（本票）與 `full_text_search`（票 37），`try_start_runtime` 兩個參數都接；`baton-api`／`baton-domain`／`baton-store` 的模組與匯出、`baton-client` 與 Tauri 的型別匯入與指令（生命週期、`app_quit`、知識）、Web UI `AppClient` 的 HTTP／Tauri 實作（生命週期與驗證方法）、`App.tsx`（`ExitProgress` 與 `KnowledgePage`）、`fixtures.ts`、`App.test.tsx` 的假用戶端（工作面板加 `verifications: []`，並保留生命週期方法）、開發指南的 crate 表、章節與情境測試清單都合併為同時描述三張票。`lifecycle_client` 測試的契約補上票 13 新增的必填欄位 `verification_checks: vec![]`（斷言不變）。(4) 停止意圖生效期間不阻擋確定性檢查：驗證不是新 Run，不在票 25「新派工、交接與重試」的阻擋範圍；Runtime 結束時驗證命令隨 `kill_on_drop` 終止，重新開啟後依票 13 的規則重新執行檢查（AI-153）。本次只整合、不新增行為。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票（與 AI-117、AI-119、AI-175、AI-176、AI-178 同）。
- 替代方案：停止意圖生效時也暫停驗證（屬新行為，超出整合範圍；需要時另開票）。
- 影響：已用本票分支建過的開發資料庫，`lifecycle` 記在第 16 或 17 版，與整合後的編號不一致，需要刪除重建。
- 審閱：未審

### AI-180：單次批准（E1）綁定的五項內容：說明欄位不列入、成果版本取 Task 已登記成果的最新版本
- 重要度：高
- 時間／階段：2026-10-22，票 23 實作
- 決策：權限提示的裁定請求建立時就算好並保存它綁定的邏輯操作（`baton_domain::approval::ApprovalBinding`，存在請求的 `permission` JSON 內，不新增資料表）：(1) **操作識別**＝工具名稱，Bash 看得懂的單一指令另加程式與子命令（`Bash:git push`）；組合指令或無法斷詞時只有 `Bash`、整串指令列入重要參數；(2) **目標資源**＝`file_path`／`notebook_path`／`url`／`path`／`query` 的第一個，Bash 則是子命令之後非旗標的字詞（`origin main`）；(3) **重要參數**＝其餘輸入（含 Bash 的旗標、`timeout`、`run_in_background`）；(4) **要求雜湊**＝「工具名稱＋輸入」的 SHA-256；(5) **成果版本**＝提出請求的 Task 在 `register_artifact` 登記的每個成果的最新版本，`名稱@版本` 依名稱排序（沒有成果時是空字串）。Claude Code 為工具附上的 `description` 說明欄位不列入綁定與雜湊。綁定只含操作本身，不含 Run、帳號或時間。
- 理由：驗收條件要求「換帳號或 Run 後仍可沿用」，而新的 Run 幾乎一定會重寫 Bash 的 `description`；票 22 用完整輸入當指紋，換 Run 後的批准實際上用不到。「成果版本」在本專案唯一可觀察的版本化產出是 Artifact 版本（票 12）；把 Task 的全部成果版本一起綁定，Worker 在批准後改了要推送或發佈的內容就不能沿用。
- 替代方案：以 git HEAD 當成果版本（Runtime 不讀 worktree，詞法判定也不碰檔案系統）；只綁定與操作目標同名的成果（無法從指令可靠推得）；把 `timeout` 等也視為說明欄位（改 timeout 也可能改變結果，保守列入）。
- 影響：Worker 在批准後登記任何新的成果版本，同一個 Task 尚未使用的「允許本次操作」都會失效、要重新裁定；研究或文件 Task 頻繁登記成果時會多問。程式碼 Task 若只改 worktree、不登記成果，成果版本不會變，要靠要求雜湊與參數比對。
- 審閱：未審

### AI-181：執行時逐項核對；不符時不消耗批准、改建新請求並回報原因；沒有綁定的舊批准不再可用
- 重要度：高
- 時間／階段：2026-10-22，票 23 實作
- 決策：Worker 再次提示時，以要求雜湊找出同一個 Task 的同一個操作；已回答、尚未使用的裁定由新到舊檢查：「拒絕」一律適用（消耗一次，回 `denied`）；「允許本次操作」要五項全部相符才消耗（回 `approved`）。任何一項不同（或同一種操作但目標、參數不同）時**不消耗**那個批准，改建新的裁定請求並回 `waiting_decision`，回應帶 `approval_check: {decision_request_id, result}`（`operation_changed`／`target_changed`／`params_changed`／`request_hash_changed`／`artifact_version_changed`），新請求的 `permission.rejected_approval` 與證據也說明哪一項變了。票 23 之前建立、沒有綁定的請求，「允許這一次」保守地不再使用（待處理的去重仍以舊指紋比對）。批准只綁 Task（沿用 AI-139），其他 Task 的同一個操作與它無關。
- 理由：驗收條件「參數或成果版本改變時拒絕」；保留原批准讓 Worker 回到原本的操作時仍可執行一次（被拒絕的嘗試不該吃掉使用者的批准）。明確回報原因，Worker 才知道不要換個寫法重試。
- 替代方案：不符時直接回 `denied`（Worker 卡住，使用者也看不到新的要求）；不符時作廢原批准（使用者要重複批准原本已同意的操作）；舊批准照舊用指紋比對（沒有成果版本可以核對）。
- 影響：開發資料庫中票 22 期間已回答但還沒用掉的「允許這一次」會變成要重新裁定。
- 審閱：未審

### AI-182：「結果未知時先查證」：用過的批准一律視為結果未知，再次提出同一個操作時建立要求先查證的請求
- 重要度：高
- 時間／階段：2026-10-22，票 23 實作
- 決策：Adapter 目前沒有工具執行結果的事件（`AdapterEvent` 只有啟動、心跳、訊息、限流、用量、結束），Runtime 無法知道放行之後操作是否成功，所以**每個用過的單次批准都視為結果未知**。同一個 Task 再次提出同一個操作（要求雜湊相同）時：不沿用、建立新的裁定請求，問題改為「先前已批准並放行過一次，結果未知。查證之後，要再允許一次嗎？」，`recommendation` 一律是 `deny`，選項文字改為「已查證，允許再執行一次」／「拒絕（不要重送）」，請求帶 `permission.prior_use`（哪個批准、哪個 Run、何時使用），Worker 收到 `approval_check.result = used_result_unknown` 與「不要直接重送，先查證」的訊息。查證由使用者（或 Worker 改做查證步驟）完成，Runtime 不自動查證。
- 理由：GLOSSARY 的 One-off Approval 與 Unknown Side Effect 都要求「結果未知時先查證，不盲目重送」；D-15 也把「結果未知外部操作的處置」列為必須送裁定。在看不到結果的前提下，最保守的作法是把每次重送都交給使用者並預設建議拒絕。
- 替代方案：新增 Adapter 的工具結果事件，區分「已完成」與「未知」（屬票 14 的 Adapter 解析，範圍外）；新增 Agent 工具讓 Worker 自報結果（Agent 自述不可當作查證依據）；用過之後就和一般請求一樣重新詢問（使用者看不出這是重送）。
- 影響：Worker 合理地重跑同一個操作（例如第一次推送失敗後重推）也會看到「結果未知、建議拒絕」的請求，使用者要自己判斷。票 14 若加入工具結果事件，可把「已確認完成」的情形改為一般的重新詢問、「已確認失敗」的情形改為不需查證。
- 審閱：未審

### AI-183：「修改 Workspace 授權」：摘要列出變動與影響，擴大時須 `acknowledged_expansion`；修改授權不回答裁定請求
- 重要度：中
- 時間／階段：2026-10-22，票 23 實作
- 決策：(1) 授權確認摘要（`POST …/authorization/summary`）在已有生效授權時多回 `changes`：逐項列出相對於目前 revision 新增／移除的能力、目標與限制，`expands`（新增能力、新增目標或移除限制）與 `impact`（「這是修改 Workspace 授權，不是單次批准…套用到所有 Task」「範圍內的操作之後會自動允許」「執行中的 Worker 下一次權限提示就依新授權判定」、高風險能力的提醒）。(2) `PUT …/authorization` 新增 `acknowledged_expansion`；修改既有授權而且擴大時必須為真，否則 400 `authorization_expansion_not_acknowledged`。第一次確認（由 I2 的確認摘要與 `acknowledged_risk` 把關）與只縮小授權不需要。(3) 裁定中心的權限請求把「允許本次操作」與「修改 Workspace 授權」分成兩個區塊：後者只有在請求的能力尚未授權時，提議「目前授權＋這個能力（目標為 Workspace 目錄）」並顯示摘要的變動與影響，勾選「我了解這會擴大 Workspace 授權」後才能保存；能力無法歸類時不提供，能力已授權但目標或限制不涵蓋時導向授權設定頁。(4) 修改授權**不會**回答那個裁定請求：Task 仍等待使用者回答「允許本次操作」或「拒絕」；之後同類操作依新授權自動允許。授權設定頁編輯既有授權時同樣顯示變動與影響並要求勾選。
- 理由：驗收條件「修改 Workspace 授權須顯示新增能力與影響並明確確認」；在 API 層強制確認，不只靠 UI。兩個動作分開，一次性批准才不會變成擴權，擴權也不會冒充成某個請求的答案。
- 替代方案：只在 UI 要求勾選（其他用戶端可以略過）；修改授權後自動以「允許」回答請求（把兩個動作混在一起）；在裁定中心提供能任意編輯目標與限制的完整授權編輯器（與授權設定頁重複）。
- 影響：既有的整合測試與用戶端在擴權時都要帶 `acknowledged_expansion`（Tauri 指令的引數是選填，預設 false）。使用者修改授權後還要再回答一次請求，多一步。
- 審閱：未審

### AI-184：票 23 整合 rebuild/v1（票 13、37）：AI 編號兩次順延、開發指南兩邊都保留
- 重要度：低
- 時間／階段：2026-10-22，票 23 實作（run R057-23-impl）
- 決策：整合分支在本票開發期間先後併入票 13（AI-148～AI-158，migration 16）與票 37（AI-159～AI-167，migration 17），所以本票的 AI 條目由 AI-148～AI-151 順延為 AI-159～AI-162、再順延為 AI-168～AI-171（內容只改編號），開發指南的引用同步改號；整合分支既有條目不動。本票沒有新增 migration（綁定存在 `decision_request.permission` 的 JSON 內）。`docs/development.md` 的 crate 表與測試清單衝突一律兩邊都保留（`approval.rs` 與 `knowledge.rs`、`one_off_approval.rs` 與 `knowledge.rs` 的情境測試）。
- 理由：不可修改整合分支已有的 AI 條目；功能彼此獨立。
- 替代方案：保留原編號（與整合分支重號）。
- 影響：無行為變更。
- 審閱：未審

### AI-185：票 23 整合：第三次整合 rebuild/v1（票 25）：AI 編號再順延為 AI-180～AI-184、開發指南兩邊都保留
- 重要度：低
- 時間／階段：2026-10-22，票 23 整合（run R068-23-integrate）
- 決策：(1) rebuild/v1 在本票驗證期間併入票 25（AI-168～AI-179，migration 18 `lifecycle`），本票的 AI-168～AI-172 順延為 AI-180～AI-184，移到 AI-179 之後，內容不變；開發指南的引用（`PUT …/authorization` 列的 AI-171、單次批准段落的「AI-168～AI-170」）同步改為 AI-183、AI-180～AI-182；AI-184（原 AI-172）內文記錄的歷史編號保留原文。程式碼與票面沒有引用這些編號。自動合併時 AI-184（原 AI-172）的 `- 審閱：未審` 行與票 25 的 AI-168 標題相鄰而被吃掉，重建時補回。(2) 本票沒有新增 migration，票 25 的 migration 18 照原樣套用。(3) `docs/development.md` 的衝突兩邊都保留：crate 表的 `baton-domain` 列以整合分支為準（含 `lifecycle.rs`），加回本票的 `approval.rs`；情境測試清單以整合分支為準（含 `lifecycle.rs`、`lifecycle_client.rs`），加回本票的 `one_off_approval.rs`。(4) 依驗證 R065 的要求檢查票 25 新增的程式與測試：沒有建構 `ConfirmRequest`，也沒有呼叫 `PUT …/authorization` 擴大既有授權，所以不需要補 `acknowledged_expansion`。本次只整合、不新增行為。
- 理由：整合分支既有的 AI 條目編號不可改動，順延尚未合併的本票（與 AI-172、AI-179 同）。兩張票的功能彼此獨立。
- 替代方案：保留原編號（與票 25 重號）。
- 影響：無行為變更。
- 審閱：未審

### AI-186：備份存放在資料目錄的 `backups/`，成果以雜湊命名的副本共用；不新增 migration
- 重要度：中
- 時間／階段：2026-10-22，票 40 實作
- 決策：每份備份是 `backups/<13 位數毫秒>-<觸發原因>/`，內含 Online Backup API 的快照 `control.sqlite` 與 `manifest.json`（觸發原因、時間、schema 版本、快照雜湊、快照中登記的全部成果版本）；成果檔案複製成 `backups/blobs/<SHA-256>`，多份備份共用；最新一次演練結果寫在 `backups/drill.json`。備份清單與演練紀錄都只存在檔案系統，不進控制資料庫，所以本票沒有 migration。備份引擎放在 `baton-store`（需要 SQLite 連線），直接用 `std::fs`，不經 WorkspaceFs Port。
- 理由：備份的清單必須在控制資料庫損毀或被還原時仍然可讀，放在資料庫裡就失去意義；成果不可變，以雜湊命名可讓 7＋4 份備份不重複佔空間；不加 migration 可避開與並行票的版本號衝突。快照與資料庫檔案同屬持久化層，與 Store 直接開 SQLite 檔案的做法一致。
- 替代方案：備份紀錄存進控制資料庫的新資料表（需要 migration，且還原後的紀錄是舊的）；每份備份各自完整複製成果（簡單但空間成本是 11 倍）；只記錄成果路徑而不複製（成果儲存損毀時備份無用）。
- 影響：備份目錄可直接整個複製到別處保存；harness 以真實檔案系統驗證。成果副本只在內容雜湊相符時沿用，損毀的副本會在下一次備份時以成果檔案重寫。
- 審閱：未審

### AI-187：備份不含任何憑證與登入資料：只備份控制資料庫與成果，並清空 `credential` 表
- 重要度：高
- 時間／階段：2026-10-22，票 40 實作
- 決策：備份只包含控制資料庫快照與被登記的成果檔案。Profile 的原生設定目錄（`profiles/`，含 `.credentials.json`）、DPAPI 保護的桌面憑證檔（`auth/`）、Run 設定快照（`run-settings/`）與日誌都不在備份內。快照中的 `credential` 表（token 雜湊與範圍）在快照上清空並 VACUUM，被刪除的內容不殘留在空閒頁面。情境測試逐位元組掃描備份目錄，確認原生登入 token、登入檔內容、桌面與 Agent 憑證的原文與雜湊、憑證檔內容都不存在。
- 理由：票面要求「備份不含登入秘密」，驗收要求掃描測試；token 雜湊雖然不是原文，但仍是憑證資料，而且還原後 Runtime 啟動時本來就會輪替桌面憑證、舊 Run 也不該再回報，保留它們沒有用處。
- 替代方案：保留 `credential` 表（還原後舊 Agent 憑證在到期前仍然有效）；連 Profile 目錄一起備份（會帶走原生登入資料，違反票面）。
- 影響：還原後每個 Profile 都要重新核對；若原本的 Profile 目錄不見了，需要重新登入。舊 Run 的 Agent 憑證在還原後一律 401。
- 審閱：未審

### AI-188：還原只寫入空目錄；備份當下尚未結束的 Run 在還原時一律結束，派工佇列保留
- 重要度：高
- 時間／階段：2026-10-22，票 40 實作
- 決策：`restore_backup`（與 `baton-runtime restore <備份目錄> <目標資料目錄>`）只接受空目錄或不存在的目標。資料庫完整時，還原會套用 migration，再把狀態是 starting／running 的 Run 以 `Interrupt` 標為中斷（原因以 `restored_from_backup` 開頭）、queued 的 Run 以 `RequestCancel` 取消（Domain 的狀態機不允許 queued → interrupted，沿用既有的「尚未啟動立即取消」）；兩者都釋放寫入所有權、Outbox 視為處理完畢、Task 回到 ready。派工佇列（`dispatch_request`）不動。
- 理由：還原的 Runtime 不知道備份之後外部世界發生了什麼；若讓 Supervisor 依舊狀態找回或重新啟動程序，就是重放外部操作。直接結束、交給使用者重新派工，與「結果不明的外部操作不重送」（票 10）一致。派工佇列只是「使用者想做的事」，還沒有產生任何外部操作，保留它比較不會丟失使用者意圖。
- 替代方案：新增 Domain 命令或允許 queued → interrupted（要改狀態機與既有測試）；連派工佇列一起清空（更保守，但使用者要逐一重新派工）；在備份時就改寫快照（備份就不再是忠實的快照）。
- 影響：還原後 Runtime 啟動時，排隊中的 Task 會照常被 Scheduler 派出新的 Run；還原時被取消的 queued Run，事件原因是 `cancelled_before_launch`，看不出是還原造成的（Run 的數量記在演練結果的 `interrupted_runs`）。
- 審閱：未審

### AI-189：觸發規則的解讀：每日 = 距上一份滿 24 小時；每個正式裁定都是「重要裁定」；一小時合併 = 距上一份滿一小時才執行
- 重要度：中
- 時間／階段：2026-10-22，票 40 實作
- 決策：每日備份在距離上一份備份（不論原因）滿 24 小時時建立。每個正式裁定（`POST …/decisions/{id}/answer` 成功，含權限提示的裁定與冪等重送）都要求一次備份；距離上一份備份已滿一小時就在下一次處理時立刻備份，否則等到滿一小時，期間的要求合併成一份。Stage 完成也走同一個入口（`BackupScheduler::request_milestone_backup`），但 Stage 在第四階段才有，目前沒有呼叫端。待處理的要求只存在記憶體。背景迴圈每分鐘檢查一次。
- 理由：裁定中心（票 22）沒有「重要度」欄位，自己挑哪些算重要容易漏；有一小時的合併上限，全部都算的成本可控。「滿一小時才執行」讓最後一份備份涵蓋同一小時內的所有裁定。要求放記憶體可避免為此新增 migration，最壞情況由下一次每日備份涵蓋。
- 替代方案：只把 Agent 提出的裁定算重要、權限提示不算；以日曆小時（整點）合併；把待處理要求持久化到資料庫或檔案。
- 影響：Runtime 在要求後一小時內重啟，那次裁定最晚要等到下一次每日備份才被備份。
- 審閱：未審

### AI-190：保留規則的解讀：7 個有備份的 UTC 日子與 4 個有備份的週各留最新一份，另留最新一份 migration 前備份
- 重要度：中
- 時間／階段：2026-10-22，票 40 實作
- 決策：每次備份後套用保留規則：由新到舊，最近 7 個「有備份的 UTC 日子」各留當天最新的一份（每日），最近 4 個「有備份的週」（UTC，星期一開始）各留該週最新的一份（每週），另外永遠留下最新一份 migration 前的備份；其餘刪除，沒有被保留備份引用的成果副本一起刪除。同一天的多份備份（例如每日與重要裁定）只留最新那份。
- 理由：spec 只說「7 份每日與 4 份每週」；以「有備份的日子」計算，Runtime 關閉幾天也不會因為時間流逝把所有備份清光。migration 前的備份是升級出問題時唯一能回到舊 schema 的備份，如果當天稍後又有備份就被取代，就失去用途。
- 替代方案：以日曆的最近 7 天、4 週計算（長時間未開機後可能一份不剩）；migration 前的備份與其他備份一樣處理；重要裁定的備份另外保留。
- 影響：備份總數最多約 12 份（7＋4，與 migration 前那份可能重疊）。同一天較早的重要裁定備份會被當天較晚的備份取代。
- 審閱：未審

### AI-191：migration 前的備份失敗時不套用 migration，Runtime 拒絕啟動
- 重要度：中
- 時間／階段：2026-10-22，票 40 實作
- 決策：`Store::open` 發現既有資料庫需要套用 migration（已套用版本 > 0 且 < 程式支援的版本）時，先以寫入連線建立 migration 前的備份；備份失敗就回報 `StoreError::PreMigrationBackup`，不套用任何 migration，Runtime 啟動失敗。新建的資料庫與已是最新版本的資料庫不備份；比程式新的資料庫照舊由 `IncompatibleSchema` 處理。
- 理由：migration 是不可逆的結構變更；沒有備份就升級，一旦 migration 出錯就沒有退路。啟動失敗會顯示原因（例如磁碟已滿），比悄悄升級安全。
- 替代方案：備份失敗只記日誌、照樣升級（可用性較高，但違背 migration 前備份的目的）。
- 影響：磁碟空間不足時，新版程式在使用者騰出空間之前無法啟動。
- 審閱：未審

### AI-192：還原演練的做法：第一份備份後立即演練、之後每週；演練最新備份；結果只保留最新一次
- 重要度：低
- 時間／階段：2026-10-22，票 40 實作
- 決策：演練從沒做過、或上次演練已滿 7 天時進行（所以第一份備份之後的第一次處理就會演練）。演練把最新的備份還原到 `backups/.drill/`（與正式資料目錄隔離，前後都整個刪除），核對五項：還原（快照雜湊與 manifest 相符）、`integrity_check`、`foreign_key_check`、成果檔案大小與雜湊、資料庫登記的成果版本與 manifest 一一對應；任何一項沒過就是不通過。結果寫進 `drill.json`，只保留最新一次，設定頁顯示。成果檔案在備份時已缺少或損毀，備份照常完成，由演練回報成果雜湊不一致。
- 理由：第一份備份是否可用越早知道越好；只演練最新的那份，因為它是最可能被拿來還原的。單一個損毀的成果不該讓資料庫本身也沒有備份。
- 替代方案：演練放在系統暫存目錄（跨磁碟時複製較慢，且不在 Baton 資料目錄的管理範圍）；保留演練歷史；成果損毀時讓備份失敗。
- 影響：每週會有一次完整複製最新備份的 I/O；演練歷史看不到。
- 審閱：未審

### AI-193：「備份期間暫停成果清理」目前沒有可暫停的對象；異地副本與還原 UI 不在本票
- 重要度：低
- 時間／階段：2026-10-22，票 40 實作
- 決策：目前沒有任何會刪除成果檔案的機制（票 12 明定未登記檔案不自動刪除），所以本票沒有實作暫停的開關；備份排程本身以互斥鎖依序進行，保留規則刪除舊備份時不會與備份或演練並行。日後新增成果清理時，必須與 `BackupScheduler::tick` 互斥。spec D-18 的「使用者另外指定的額外備份目錄」沒有列在票面，本票不做；還原只有 `baton-runtime restore` 指令，設定頁只顯示狀態，沒有操作按鈕。
- 理由：為不存在的清理機制先做暫停開關是推測性的功能；異地副本與還原 UI 都不在票面的驗收條件內。
- 替代方案：先在 Store 加一個「備份中」旗標給未來的清理機制使用；設定頁加「立即備份」「立即演練」「還原」按鈕。
- 影響：使用者需要在命令列還原；做成果清理的票要記得處理與備份的互斥。
- 審閱：未審

### AI-194：票 40 整合 rebuild/v1（票 22、13、37）：AI 編號順延為 AI-168～AI-175
- 重要度：低
- 時間／階段：2026-10-22，票 40 實作
- 決策：實作期間 rebuild/v1 先後合入票 22（AI-138～AI-147）、票 13（AI-148～AI-158）與票 37（AI-159～AI-167），本票的八個條目改為 AI-168～AI-175，內容不變，放在 AI-167 之後。本票沒有 migration，所以沒有 migration 版本需要順延；整合票 22 後把裁定中心的正式裁定接上備份要求（AI-171）；整合票 37 後 `RuntimeOptions` 同時有 `background_backup` 與 `full_text_search`。
- 理由：AI 編號只能往後追加，並行的票先合入的保留原編號。
- 替代方案：保留原編號（會與已合入的條目重複）。
- 影響：本票較早的 commit 訊息中提到的 AI-148～AI-155 指的是改號前的編號。第二次整合（R070-40-integrate）後本票條目再順延為 AI-186～AI-194，本條目內的編號是當時的編號（見 AI-195）。
- 審閱：未審

### AI-195：票 40 整合 rebuild/v1（票 25、23）：AI 編號再順延為 AI-186～AI-194；Runtime 同時有備份排程與完全退出
- 重要度：中
- 時間／階段：2026-10-22，票 40 整合（R070-40-integrate）
- 決策：(1) 整合分支先後併入票 25（`lifecycle`，migration 18；AI-168～AI-179）與票 23（單次批准，沒有 migration；AI-180～AI-185），本票條目原本的 AI-168～AI-176 順延為 AI-186～AI-194，移到 AI-185 之後，內容不變；條目內的交互引用改成新編號，AI-194（原 AI-176）記錄的是第一次整合當時的編號，保留原文並加註。整合途中 rebuild/v1 又併入票 23，所以分兩次合併：第一次（只含票 25）的合併 commit 暫時把本票編成 AI-180～AI-189，第二次合併後才定為現在的編號。本票沒有 migration，不需順延；整合分支的 18 個 migration 都會套用，migration 前備份照舊在既有資料庫需要升級時觸發。程式碼、開發指南與票面都沒有引用這些編號（開發指南中的 AI-180～AI-183 是票 23 的條目）。(2) 衝突兩邊都保留：`RuntimeOptions` 同時有 `background_backup`（本票）與 `exit_timing`／`exit_linger`（票 25），`RunningRuntime` 同時持有備份排程（`backups`、`backing_up`）與完全退出的執行者（`exiting`、`exits`），`shutdown`／`Drop` 兩者都收尾；測試 harness 同時關閉 Supervisor 與備份的背景迴圈並傳入退出時間參數，`run_backups`、`request_milestone_backup`、`backups_dir` 與 `lifecycle_file_path`、`wait_for_exit` 並存；`baton-api`／`baton-domain` 的模組說明、Tauri 的 `baton_api` 匯入、`fixtures.ts`（備份摘要、演練結果與生命週期）都合併；開發指南的 crate 表以整合分支的列為準（含票 23 的 `approval.rs`），再加回本票的 `backup.rs`、`/api/v1/backups` 與 `baton-runtime restore`，「備份與還原演練」與「桌面生命週期與完全退出」兩節並存。(3) 備份的背景迴圈與其他背景工作一樣監聽同一個停止訊號，完全退出結束服務時隨之結束；完全退出期間不另外暫停備份。本次只整合、不新增行為。
- 理由：已進入整合分支的條目編號不可改動，順延尚未合併的本票（與 AI-179 同）。備份不建立 Run，也不啟動外部程序，不在票 25「新派工、交接與重試」的阻擋範圍。
- 替代方案：停止意圖生效時也暫停備份（屬新行為，超出整合範圍；需要時另開票）。
- 影響：本票較早的 commit 訊息與 `.orchestration` 下的驗證紀錄（R067）提到的 AI-168～AI-176，以及第一次合併 commit 中的 AI-180～AI-189，都是改號前的編號。
- 審閱：未審

### AI-196：真實 Claude Code Adapter 的啟動參數：不載入任何設定檔來源、權限經 permission_prompt、Prompt 經標準輸入
- 重要度：高
- 時間／階段：2026-10-22，票 14 實作
- 決策：Worker 以 `claude -p --output-format stream-json --verbose --setting-sources "" --strict-mcp-config --mcp-config <Run 的 mcp.json> [<快照的 mcp.json>] --allowedTools mcp__baton --permission-prompt-tool mcp__baton__permission_prompt [--settings …] [--append-system-prompt-file …] [--model …]` 啟動。Prompt 寫成 Run 目錄的 `prompt.md` 經標準輸入交付。權限模式維持 CLI 預設（不使用 `bypassPermissions`），只預先允許 Baton 自己的 MCP 工具，其餘需要核准的操作經票 22 的 `permission_prompt` 交給 Runtime 判定。執行檔是 `claude`（CreateProcessW 依 PATH 找 `claude.exe`），MCP shim 是與 Runtime 同目錄的 `baton-mcp-shim.exe`。
- 理由：`--setting-sources ""` 讓使用者層與 Workspace 內的 `.claude\settings*.json` 都不載入。實測 `project,local` 時 Workspace 設定的 `defaultMode: acceptEdits` 會生效，而 P3 授權下 Worker 可以寫這些檔案（票 22 驗證 R052 的意見），等於 Worker 能改自己的權限；設定改為只來自 Run 的唯讀快照（D-09）。`--strict-mcp-config` 讓 Worker 只看到 Baton 的 shim 與快照中的共同 MCP 伺服器（D-05）。Prompt 經標準輸入可避開 Windows 指令列長度上限，也不會出現在程序清單。全部參數已在 Claude Code 2.1.307 實測（見 AI-204）。
- 替代方案：`--setting-sources project,local`（Worker 可藉 Workspace 設定放寬權限）；`--bare`（會改變認證來源的讀取方式）；`bypassPermissions`（違反 D-08）。npm 安裝的 `claude.cmd` 不能直接以 CreateProcessW 啟動，目前不支援，需要時改為經 `cmd.exe` 或解析實際路徑。
- 影響：repo 自己的 `.claude\settings.json`（hooks、權限、環境變數）對 Worker 不生效；需要的設定要放進共同設定。
- 審閱：未審

### AI-197：啟動 Worker 前移除繼承的 `ANTHROPIC*`、`CLAUDE*` 環境變數
- 重要度：高
- 時間／階段：2026-10-22，票 14 實作
- 決策：Runtime 啟動時記下繼承到的環境變數名稱；啟動 Worker 前移除名稱以 `ANTHROPIC` 或 `CLAUDE` 開頭（不分大小寫）的全部變數，只保留 `CLAUDE_CODE_GIT_BASH_PATH`，再設定 `CLAUDE_CONFIG_DIR` 為 Run 帳號 Profile 的目錄。移除以新增的 `SpawnSpec.env_remove` 交給 ProcessControl，憑證以 `env` 交付，都不進指令列或 `command_hash`。
- 理由：`ANTHROPIC_API_KEY`、`ANTHROPIC_AUTH_TOKEN`、`CLAUDE_CODE_OAUTH_TOKEN`、`CLAUDE_CODE_USE_BEDROCK／VERTEX`、`ANTHROPIC_BASE_URL` 等都會讓實際帳號與 Profile 脫鉤；Runtime 若是從 Claude Code session 中啟動，還會繼承 `CLAUDECODE` 等巢狀標記。比照 AI-02 以前綴清除最不容易漏；Git Bash 路徑只影響 Windows 上找 Bash，與認證無關。
- 替代方案：只移除明列的變數（新版本加入的認證變數會漏掉）；清空整個環境（會失去 PATH、USERPROFILE 等必要變數）。
- 影響：使用者若以環境變數調整 Claude Code 行為（例如 `CLAUDE_CODE_MAX_OUTPUT_TOKENS`），對 Worker 不生效；需要時應改放共同設定或擴充保留清單。
- 審閱：未審

### AI-198：CLI 輸出寫入 Run 目錄的檔案，Adapter 追蹤檔案取得事件
- 重要度：中
- 時間／階段：2026-10-22，票 14 實作
- 決策：ProcessControl 依新增的 `SpawnSpec.stdin_file`／`stdout_file`／`stderr_file`，把 `claude` 的標準輸出寫入 `<資料目錄>\runs\<run_id>\stream.jsonl`、標準錯誤寫入 `stderr.log`（share read/write/delete）。Adapter 的事件串流每 250 毫秒讀取檔案新內容並逐行解析，有新輸出就送一次 `Heartbeat`。Runtime 重建後重新接上同一個檔案，從頭重播。
- 理由：程序由 Job Object 管理，Runtime 結束或當掉時程序繼續執行（D-10；關閉 Job handle 不殺程序），管線在 Runtime 重建後無法接回，檔案可以。重播的 Session 綁定是冪等的，heartbeat 只會續約 lease，過期後仍由 ProcessControl 查證存活。
- 替代方案：具名管線（重建後無法取回已寫出的內容）；持久化已處理的位移（多一份狀態，目前沒有必要）。
- 影響：重建後重播會重送限流與用量事件，票 27 的觀測會多記一次同一筆用量（推估偏高、偏保守）；stream 檔案隨 Run 保留在資料目錄，目前沒有清理流程。
- 審閱：未審

### AI-199：stream-json 事件對應：限流訊號、用量與錯誤
- 重要度：中
- 時間／階段：2026-10-22，票 14 實作
- 決策：`system/init` → `Started`（`session_id`）；`assistant` 的文字區塊 → `Message`。`rate_limit_event`：`status` 的 `allowed`／`allowed_warning`／`rejected` 對應票 27 的 `Allowed`／`Warning`／`Exhausted`（其他狀態不轉換），屬於 `rateLimitType` 的視窗（不認識的視窗為 `None`），`used` 取 `utilization`，沒有時取 `unifiedWindows` 中該視窗的值；`unifiedWindows` 的其他視窗以 `Allowed` 加已用比例各送一個訊號。`result`：有 `usage` 時先送 `UsageReported`，`tokens` = 輸入 + 輸出 + 快取建立（快取讀取不計，採用 AI-121 的建議）；`is_error` 為 false 且 `subtype` 為 `success` 才是成功，否則先送新增的 `AdapterEvent::Error`（`result` 文字，500 字元內）再送 `Exited { success: false }`。不是 JSON 的行送 `Error`。Supervisor 以最後一個錯誤作為 Run 的失敗原因。`resume_capability` 一律回報 `Unknown`。
- 理由：實測 2.1.307 的未登入與不存在的模型，`result` 都是 `subtype: success` 但 `is_error: true`，只看 `subtype` 會誤判。`unifiedWindows` 同時帶兩個視窗的已用比例，送出後票 27 才能判定「觀測完整」（AI-124）；非主要視窗的狀態 CLI 沒有說，以已用比例為準。快取讀取的 token 很多（實測一次 17140）但計費與額度權重遠低於其他類別，計入會讓推估嚴重偏高。
- 替代方案：只送主要視窗（另一個視窗永遠只有推估）；`tokens` 計入快取讀取。
- 影響：撞到限制時的實際形狀尚未實測（`docs/adapter-contract.md`）；resume 屬後續票。
- 審閱：未審

### AI-200：原生 Session 的條件綁定：不同的 Session 只記錄衝突，不改綁定也不結束 Run
- 重要度：中
- 時間／階段：2026-10-22，票 14 實作
- 決策：Run 新增 `native_session_id`。`BindNativeSession` 需要目前寫入所有者的世代；沒有綁定時綁定並記 `run_native_session_bound`；相同時沒有任何變動；不同時保留原綁定並記 `run_native_session_conflict`（含兩個識別），Run 照常執行。
- 理由：票面要求「不靜默改指其他原生 Session」。衝突在同一個 `claude -p` 程序內不該發生，若發生代表 CLI 行為改變，應留下可查證的紀錄；直接讓 Run 失敗會中斷可能仍正常的工作，是否要處置交給 Session resume 與交接的票決定。
- 替代方案：衝突即讓 Run 失敗（較保守，但沒有證據顯示工作本身有問題）；靜默忽略（違反票面）。
- 影響：UI 目前不顯示衝突事件，只能從事件表查看。
- 審閱：未審

### AI-201：Claude Code 最低版本固定為 2.1.307，版本記在 Profile 核對中，未知也阻擋
- 重要度：高
- 時間／階段：2026-10-22，票 14 實作
- 決策：新增 `baton_domain::claude_code`，最低版本 `MIN_SUPPORTED_VERSION = 2.1.307`（真實 Adapter 契約實測通過的版本）。首次啟動精靈的 `MIN_CLAUDE_CODE_VERSION` 改由它導出（原暫定 2.1.0）。核對 Profile 時同時執行 `claude --version` 並記入 `verification.cli_version`；低於最低版本是 `cli_version_unsupported`（帳號問題），找不到 Claude Code、取不到或看不懂版本是 `cli_version_unknown`（狀態不明），兩者都阻擋該 Profile 的派工並顯示原因。票 14 之前的核對沒有版本，需要重新核對。
- 理由：spec D-20 規定最低版本在 Adapter 驗證時固定；D-08 要求以版本探測決定能力、不支援時阻擋並說明，不假設支援。只有 2.1.307 實際驗證過 Adapter 用到的旗標與事件格式。CLI 是本機唯一一份，但「派工會不會成功」以 Profile 為單位判定與顯示，所以放在 Profile 的核對結果裡。
- 替代方案：最低版本設 2.1.0（未驗證 2.1.0～2.1.306 是否支援全部旗標）；每次啟動 Run 時探測版本（每個 Run 多一次程序啟動，且無法在派工前顯示原因）。
- 影響：既有的 Profile 在重新核對前都不能派工；CLI 降版後要重新核對才會被阻擋。
- 審閱：未審

### AI-202：設定快照在啟動前自動准入與核對，API 與 Supervisor 共用並依 Run 序列化
- 重要度：中
- 時間／階段：2026-10-22，票 14 實作
- 決策：新增 `baton_api::RunSettings`（取代 `materialize_run_settings`），API 的快照端點與 Supervisor 共用同一個實例，每個 Run 一把鎖。Supervisor 在 launch intent 持久化之後、簽發憑證之前取得快照：尚未綁定就以最新版本建立；已綁定就核對目錄中的檔案（含 `manifest.json`）與版本渲染結果完全相同。核對失敗以 `LaunchFailed`（`settings_snapshot_mismatch…`）結束 Run，不啟動程序；API 再要求已綁定的快照也做同樣核對，不一致回 409 `settings_snapshot_mismatch`。還沒有採用任何共同設定版本時，Run 不帶共同設定啟動。
- 理由：回應票 14 的主控意見（票 08 驗證 R040）：同一 Run 並行要求不同版本時共用 staging 目錄、已綁定的 Run 不核對目錄。D-09 要求「無法維持時阻擋並顯示，不宣稱固定成功」。尚未採用共同設定時阻擋，會讓還沒完成首次精靈匯入步驟的使用者完全無法派工；Profile 目錄本身只有登入狀態，加上 `--setting-sources ""`，不帶共同設定執行的風險可接受。
- 替代方案：未採用共同設定時阻擋派工（較嚴格）；核對失敗時以版本內容重寫目錄（可能掩蓋竄改，也與唯讀目錄的寫入規則衝突）。
- 影響：鎖只在單一 Runtime 實例內有效（Runtime 本身是單實例）；Run 啟動後快照被改動不會被偵測。
- 審閱：未審

### AI-203：模型依 Workspace 角色覆寫的資料形狀與生效時機
- 重要度：低
- 時間／階段：2026-10-22，票 14 實作
- 決策：Workspace 新增 `model_overrides { controller, worker, reviewer }`（`null` = 不覆寫），以 `PUT /api/v1/workspaces/{id}/model-overrides` 整組取代；名稱去除空白後只允許英數字與 `. _ - : [ ] @ /`、最長 100 字元。派工時把 Worker 角色的模型固定進派工 Outbox（`LaunchPayload.model`），只影響之後派出的 Run。Controller 與 Reviewer 的覆寫先保存，由各自的票使用。UI 放在 Workspace「授權設定」畫面下方。
- 理由：spec D-30。模型名稱會成為 `--model` 的引數，限制字元避免被當成其他旗標或夾帶參數；在派工時固定讓 Runtime 重建後組出相同的啟動指令。
- 替代方案：在 Run 啟動時才讀取 Workspace 的覆寫（重建後可能組出不同指令）；放進共同設定（共同設定是全域的，不能依 Workspace 區分）。
- 影響：migration `model_overrides_and_native_session` 新增 `workspace.model_overrides` 與 `run.native_session_id`（序號見 AI-205、AI-206）。
- 審閱：未審

### AI-204：真實 Adapter 契約以 Work-3 實際執行，並把輸出錄進 repo 作為 replay 對象；AI-144 的三個問題已實測
- 重要度：中
- 時間／階段：2026-10-22，票 14 實作
- 決策：契約測試有三種對象：fake、replay（真實 Adapter 讀取真實 CLI 錄下的 stream-json）、live（`#[ignore]`，手動）。本票以 Work-3（`BATON_LIVE_PROFILE_DIR=C:\Users\User\.claude-Work-3`、模型 `haiku`）實際執行 live（最後一次 5/5 通過，另有權限轉接 1/1），把兩個情境的輸出錄成 `crates/baton-harness/tests/fixtures/claude-2.1.307-*.jsonl`（路徑中的使用者名稱改成範例值）。權限轉接以本機假 Agent API 驗證 AI-144：(a) CLI 接受 `behavior` 以外的欄位，`deny` 不需要 `updatedInput`；(b) 收到 `deny` 的 Worker 沒有重試同一個操作並正常結束；(c) CLI 會帶 `tool_use_id`，Runtime 不需要它也能運作。結果記錄在 `docs/adapter-contract.md`。
- 理由：票面要求真實 Adapter 的結果記錄 CLI 版本；只有 fake 的契約無法發現 fake 與真實不一致（例如 `is_error` 與 `subtype` 的關係）。replay 讓每次 `cargo test` 都以真實輸出驗證解析器，不需要登入。執行前以 usage.py 確認 Work-3 額度足夠，未使用 Work-0；每次請求都極短。
- 替代方案：不執行 live、只寫說明（無法得知真實的錯誤輸出形狀與權限協定）。
- 影響：CLI 升版後需要依 `docs/adapter-contract.md` 重新執行 live 並更新錄製檔；`allow` 是否可省略 `updatedInput` 仍未實測（Runtime 一律帶上）。
- 審閱：未審

### AI-205：票 14 整合 rebuild/v1（票 27、18、22、13、37）：AdapterEvent 合併、migration 第 18 版、AI 編號順延
- 重要度：低
- 時間／階段：2026-10-22，票 14 實作（整合 rebuild/v1）
- 決策：整合分支已併入票 27（migration 13 `quota`）、票 18（14 `message_and_delivery`）、票 22（15 `decision_request_and_decision`）、票 13（16 `task_verification`）與票 37（17 `knowledge`），所以本票的 migration `model_overrides_and_native_session` 由第 13 個順延為第 18 個，排在最後。本票原本以 AI-121～AI-129 追加的條目與整合分支的編號重複，改為 AI-168～AI-177（內容依整合後的實作更新）。票 13 的驗證測試中自行建立的 `Run` 補上 `native_session_id`；Supervisor 同時接收設定快照服務與成果目錄；harness 的 builder 同時保留取代 Agent Adapter 與 `full_text_search` 兩個選項。`AdapterEvent` 採用票 27 的 `RateLimited { window, status, used, reset_at_millis, raw }` 與 `UsageReported`，保留本票新增的 `Error`；Supervisor 同時處理額度事件與原生 Session 綁定。前端 `App.tsx` 同時保留模型設定面板、票 22 的裁定中心與票 37 的知識分頁。裁定中心與知識分頁測試中自建的 Workspace 補上 `model_overrides`。
- 理由：不可修改整合分支已有的 migration 與 AI 條目；功能彼此獨立。
- 替代方案：合併 migration（已套用的開發資料庫會漏掉其中一部分）。
- 影響：用本票分支建過的開發資料庫（第 13 個 migration 是本票的）與整合後的編號不一致，需要刪除重建。
- 審閱：未審

### AI-206：票 14 整合 rebuild/v1（票 25、23）：migration 第 19 版、AI 編號順延為 AI-186～AI-195、完全退出找回程序改用本票的 RunSpec
- 重要度：中
- 時間／階段：2026-10-22，票 14 整合 run R069-14-integrate
- 決策：(1) 整合分支已併入票 25 的 `lifecycle`（migration 18），本票的 `model_overrides_and_native_session` 由第 18 個順延為第 19 個，排在最後；兩者沒有共用欄位（本票只對 `workspace`、`run` 加欄，票 25 新建 `runtime_lifecycle`、`workspace_lifecycle`），都會套用。(2) 整合分支已用到 AI-185（票 25 的 AI-168～AI-179、票 23 的 AI-180～AI-185），本票條目 AI-168～AI-177 順延為 AI-186～AI-195，移到票 23 條目之後；條目間的交互引用（AI-186 的「見 AI-194」、AI-193 的 migration 序號）同步更新，AI-195 決策內文記錄歷史編號的敘述保留原文。整合途中 rebuild/v1 又併入票 23，所以分兩次合併：第一次（只含票 25）的合併 commit 暫時把本票編成 AI-180～AI-189、本條為 AI-190，第二次合併後才定為現在的編號；票 23 沒有新增 migration，也沒有與本票衝突的檔案。自動合併時原 AI-177 的 `- 審閱：未審` 行與票 25 的 AI-168 標題相鄰而被吃掉，重建時補回。程式碼、文件與票面沒有引用這些編號。(3) 票 25 的完全退出要以 command_id 找回「啟動中、回執未寫回」的程序，原本用同步的 `run_spec_of`（只有 prompt 與工作目錄）；本票已把 RunSpec 改為由持久化狀態（Profile 的 CLAUDE_CONFIG_DIR、設定快照綁定、模型）非同步組出，Adapter 依此算出與啟動時相同的 command_hash，所以 `process_of` 改呼叫本票的 `run_spec`，移除 `run_spec_of`；組不出 RunSpec（例如找不到 Profile）時回傳錯誤，由停止流程記為停止異常 `stop_failed`、保留寫入所有權，而不是當作沒有程序。(4) 其餘衝突兩邊都保留：Supervisor 同時有 `run_settings`、`last_errors`（本票）與生命週期欄位（票 25），`Supervisor::new` 同時接收設定快照服務與 `LifecycleSettings`；Runtime 的 `baton_api` 匯入同時有 `RunSettings` 與 `LogSink`；harness 的 builder 同時有 `agent_adapter` 與 `exit_timing`，啟動與重啟都以 `Harness::ports()`（含取代的 Adapter）加上 `exit_timing` 呼叫 `try_start_runtime`；`WorkspaceSidebar.test.tsx` 的假用戶端同時有 `setModelOverrides` 與生命週期方法；開發指南 crate 表的 `baton-runtime` 列以整合分支為準再加回 `claude_adapter.rs`。本次只整合、不新增行為。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票（與 AI-175、AI-179 同）。找回程序時若用與啟動時不同的 RunSpec，真實 Adapter 算出的 command_hash 會不同，找不回程序，停止流程會誤把仍在跑的程序當作「未啟動」而結束 Run。
- 替代方案：保留 `run_spec_of` 只給停止流程用（真實 Adapter 下找不回程序）；組不出 RunSpec 時視為沒有程序（可能讓仍在跑的程序失去寫入所有權的保護）。
- 影響：用本票分支建過的開發資料庫，`model_overrides_and_native_session` 記在第 18 版，與整合後的編號不一致，需要刪除重建。
- 審閱：未審

### AI-207：票 14 整合 rebuild/v1（票 40）：AI 編號再順延為 AI-196～AI-206、migration 維持第 19 版、還原指令改用本票的 `production_ports(data_dir)`
- 重要度：低
- 時間／階段：2026-10-22，票 14 整合 run R073-14-integrate
- 決策：(1) 整合分支已併入票 40，用到 AI-195（票 40 的 AI-186～AI-195），本票條目 AI-186～AI-196 順延為 AI-196～AI-206，移到票 40 條目之後；交互引用（AI-196 的「見 AI-204」、AI-203 的 migration 序號見 AI-205、AI-206）同步更新，AI-205、AI-206 記錄歷史編號的敘述與標題保留原文。自動合併時原 AI-196 的 `- 審閱：未審` 行又與票 40 的 AI-186 標題相鄰而被吃掉，重建時補回；票 40 的條目逐字未動。程式碼、文件與票面沒有引用這些編號。(2) 票 40 沒有新增 migration（AI-186 備份），整合分支仍是 18 個，本票的 `model_overrides_and_native_session` 維持第 19 個，兩邊的 schema 都會套用；備份的 `schema_version` 以 `MIGRATIONS.len()` 計算，不受影響。(3) 票 40 的 `baton-runtime restore` 以無參數的 `production_ports()` 取得時鐘，本票已把它改為 `production_ports(data_dir)`（Claude Code Adapter 需要資料目錄），因此改傳還原的目標目錄；建構 Port 組合沒有副作用，還原只用到時鐘。(4) 其餘衝突兩邊都保留：`baton-runtime` 同時有 `backup` 與 `claude_adapter` 模組，`ApiState` 同時有 `run_settings` 與 `request_backup`；`App.tsx` 同時匯入備份設定與模型設定面板；開發指南 crate 表的 `baton-runtime` 列同時列出 `claude_adapter.rs` 與備份排程、還原指令。本次只整合、不新增行為。
- 理由：已進入整合分支的條目編號不可改動，順延尚未合併的本票（與 AI-205、AI-206 同）；還原指令只需要時鐘，傳目標目錄是改動最小且語意正確的做法。
- 替代方案：還原指令直接使用 `SystemClock`（要改動票 40 的匯入，與既有寫法不一致）。
- 影響：無行為變更。
- 審閱：未審

### AI-208：Guard 在 Run 准入（派工交易）時檢查；擋下的 Task 以「Guard」等待原因留在佇列，不是 409
- 重要度：中
- 時間／階段：2026-10-22，票 38 實作
- 決策：(1) 「相關 Task 准入時被檢查」實作為 `run::check_dispatch` 的一步（授權准入之後、相依之前），所以派工要求、佇列每次重試、排程總覽都經過同一個判定；pending → ready 的准入報告（`task::admission_report`）**不**檢查 Guard。(2) 違反時回 `RunError::GuardViolated`，Scheduler 把它轉成新的等待類別 `guard`（`code = guard_violated`，`WaitReason.guard` 帶 `guard_id`、`lesson_id`、`lesson_title`），派工回 202 並把 Task 留在佇列，不建立 Run、不啟動程序；修改契約（Task 退回 pending 而離開佇列）或停用 Guard（喚醒 Supervisor，佇列重試時自動派出）即解除。(3) Store 只在派工路徑載入「啟用且來源教訓現在有效」的 Guard（`Loaded.guards`），其他 Run 命令看到的是空清單。
- 理由：票面寫「Run 准入時執行適用 Guard」，而重試政策依賴該 Task 的 Run 紀錄，只有在 Run 准入時才有意義；放在 `check_dispatch` 讓所有會建立 Run 的路徑共用一個判定（與授權重新准入 AI-61 同理）。以等待原因呈現可以直接沿用工作面板既有的排隊原因 UI，並讓停用 Guard 後自動恢復，與授權、相依的處理方式一致。
- 替代方案：派工直接回 409 `guard_violated`、不排隊（UI 要另做錯誤呈現，停用 Guard 後要使用者重新派工）；同時在 pending → ready 檢查（重試政策在 ready 時無法判定，兩處檢查也可能不一致）；把違反當成 `task_admission_denied` 的缺項（會被當成契約缺漏，看不出來源教訓）。
- 影響：被 Guard 擋下的 Task 狀態仍是 `ready`，工作面板以排隊原因顯示「Guard 擋下」與來源教訓。Context Builder（票 39）或 Controller（票 16、17）若有自己的派工路徑，必須經過 `check_dispatch` 才會檢查 Guard。
- 審閱：未審

### AI-209：第一版三種 Guard 規則：契約欄位必須提到文字、禁止要求某能力、目前契約版本失敗次數上限
- 重要度：中
- 時間／階段：2026-10-22，票 38 實作
- 決策：票面的「啟動前檢查、工具限制或重試政策」各實作一種可機械判定的規則（`GuardRule`）：(1) `preflight`：六項契約欄位之一必須是「已填寫」且內容（不分大小寫）提到指定文字（1–200 字元）；(2) `tool_restriction`：Task 的 `required_capabilities` 不得含指定能力；(3) `retry_policy`：同一個 Task 在**目前契約版本**下 `failed` 的 Run 數達到上限（1–20）就不再派工，`interrupted`、`cancelled` 不算，改契約即重新計算。規則建立後不能修改（要換規則就停用舊的、建立新的）。spec 使用者故事 60 提到的「測試」形式不在第一版。
- 理由：三種都只需要派工交易裡已有的事實（契約、能力、Run 紀錄），不需要 I/O，能放在純 Domain 判定；以「契約版本」計算失敗次數，使用者依教訓改了做法（改契約）就給新的機會，不會被舊失敗永遠擋住。中斷多半是額度交接、完全退出或基礎設施問題（AI-168），不代表做法錯了，所以不計入。
- 替代方案：以正規表示式比對契約（彈性高但使用者難寫、也難解釋違反原因）；重試政策計算全部歷史失敗（改契約也無法解除）；把「測試」做成 Guard（需要在准入前執行命令，屬驗證 Adapter 的範圍）；在權限提示時攔截工具呼叫（屬操作時檢查，超出「Run 准入」）。
- 影響：重試政策是使用者針對特定教訓設的上限，與 spec D-16 的全域修正預算（票 33 等）是兩件事，兩者都會阻擋派工。工具限制只看契約宣告的能力；Worker 執行中實際使用的工具仍由權限提示依授權判定。
- 審閱：未審

### AI-210：Guard 的生效條件與適用範圍：建立即啟用、只能來自有效教訓、教訓失效則 Guard 自動失效、範圍沿用教訓
- 重要度：中
- 時間／階段：2026-10-22，票 38 實作
- 決策：(1) 只有桌面憑證能建立、啟用、停用 Guard（Agent 憑證 403）；使用者建立 Guard 的動作本身就是啟用，沒有「建立但未啟用」的草稿狀態。(2) 只有**有效**的教訓（`kind = lesson`、已驗證、在有效期間內）能轉成 Guard 或重新啟用，否則 409 `lesson_ineligible`。(3) Guard 生效 = 啟用 + 來源教訓**現在**有效；教訓被撤銷、淘汰、取代或過期後，Guard 自動不再檢查（列表標示 `active = false` 與原因），不需要使用者另外停用。(4) Guard 沒有自己的範圍，一律沿用來源教訓的範圍（全域、Workspace、Goal、Task，與 Agent 看得到的範圍相同的 `Audience::Run` 判定）。(5) Guard 不能刪除，只能停用；啟用與停用以 `expected_revision` 校驗。
- 理由：D-28「Guard 一律由使用者啟用」的重點是 Agent 不能讓 Guard 生效；使用者主動建立已經是明確同意，再多一個啟用步驟只是多一次點擊。錯誤或過時的教訓不應繼續擋工作，讓生效條件直接依賴教訓狀態，就不會出現「教訓已撤銷、Guard 還在擋」的不一致。範圍沿用教訓，避免同一條經驗有兩套範圍。
- 替代方案：建立後預設停用、另外按啟用（多一步，沒有額外保障）；教訓撤銷時連帶把 Guard 改成停用（多一個跨聚合的寫入，而且教訓狀態若之後改變不易還原）；Guard 自己可以收窄範圍（第一版不需要）。
- 影響：候選教訓必須先確認才能轉成 Guard。延長已過期教訓的有效期限（修正）之後，原本的 Guard 會自動恢復生效。
- 審閱：未審

### AI-211：重犯事件與 Guard 成效的定義：被攔下的自動記錄、未攔下的由使用者回報，兩者都指向教訓
- 重要度：中
- 時間／階段：2026-10-22，票 38 實作
- 決策：(1) 重犯事件（`lesson_recurrence`，追加式）一律指向來源教訓，分兩種：`prevented`（Guard 在 Run 准入時擋下，`Store::schedule` 在同一個交易中記錄，指向 Guard、Workspace、Task 與契約版本）與 `reported`（沒被攔下、使用者在知識分頁回報，說明必填，可指定 Task；教訓必須是已驗證的教訓，過期也可以）。(2) `prevented` 的識別由「Guard + Task + 契約版本」決定，佇列每秒重試看到同一個違反只記一次；改了契約仍違反就是新的一次。(3) 每筆同時追加 `lesson_recurrence_recorded` 事件（`aggregate_type = 'lesson_recurrence'`）供時間線追溯。(4) Guard 成效在 Guard 列表上顯示：`prevented_count`、`last_prevented_at`，以及 `reported_count`（Guard 建立之後，同一個教訓的使用者回報數）。
- 理由：票面要求「記錄重犯事件與 Guard 成效」「重犯事件可追溯到教訓」；只記攔下次數看不出 Guard 是否有漏洞，加上使用者回報的漏網次數，才算得出 spec 使用者故事 93 的「教訓重犯率」。以契約版本去重，避免佇列重試把一次重犯算成上千次。
- 替代方案：每次被擋都記一筆（數字被佇列重試灌大）；重犯事件掛在知識條目的事件流上（會讓條目的 `revision` 變動，干擾知識分頁的 `expected_revision` 校驗）；由 Agent 回報重犯（Agent 的判斷不可信，且 Agent 工具集未包含此項）。
- 影響：成效目前只有次數，沒有比率或時間序列圖；觀測面板（spec 使用者故事 93）可由 `lesson_recurrence` 直接計算。被擋下的 Task 在依舊契約版本下停用 Guard 再重新啟用時，不會再記一次重犯。
- 審閱：未審

### AI-212：Guard API、資料表與桌面殼指令的形狀（migration 20 `guard`）
- 重要度：低
- 時間／階段：2026-10-22，票 38 實作
- 決策：管理 API（桌面憑證）：`GET /api/v1/guards?workspace_id=`（來源教訓是全域或屬於該 Workspace 的 Guard，附來源教訓、`active`、`inactive_reason`、`description` 與成效）、`POST /api/v1/guards`（`{ lesson_id, rule }`）、`POST /api/v1/guards/{id}/enable|disable`（`{ expected_revision }`）、`GET|POST /api/v1/knowledge/{id}/recurrences`。錯誤代碼：`guard_invalid`（400）、`guard_not_found`（404）、`lesson_ineligible`、`guard_revision_conflict`（409）。資料表 `guard`（觸發器禁止刪除）與 `lesson_recurrence`（追加式）放在第 20 個 migration。Tauri 指令 `guard_list`、`guard_create`、`guard_enable`、`guard_disable`（`id`、`expected_revision` 平鋪為參數）、`knowledge_recurrences`、`knowledge_report_recurrence`。Web UI 的工具限制下拉選單的能力名稱照抄 Rust `Capability::label`。
- 理由：沿用知識 API 的分流、錯誤格式與 expected_revision 慣例；重犯事件屬於教訓，所以掛在知識條目之下。
- 替代方案：Guard 掛在 `/api/v1/knowledge/{id}/guards` 之下（列表要跨教訓查詢，不自然）；Tauri 指令照 `knowledge_confirm` 以 `request` 物件包 `expected_revision`（兩種寫法都可，選了較短的）。
- 影響：整合 rebuild/v1 時，若其他票先佔用第 20 個 migration，本票順延（與 AI-165 同）。能力名稱改動時 Web UI 的對照表要同步。
- 審閱：未審

### AI-213：票 38 整合 rebuild/v1（票 23、40）：AI 編號順延為 AI-208～AI-212；migration 不變
- 重要度：低
- 時間／階段：2026-10-22，票 38 實作（結束前合併 rebuild/v1）
- 決策：整合分支在本票實作期間併入票 23（AI-180～AI-185）與票 40（AI-186～AI-195），本票條目原本的 AI-180～AI-184 順延為 AI-208～AI-212，移到票 40 條目之後；條目之間沒有互相引用，程式碼、文件與票面也沒有引用這些編號。兩張票都沒有新增 migration，本票的 `guard` 維持第 19 版。其餘衝突兩邊都保留：`baton-api` 的模組說明（備份狀態與 Guard 端點）、Web UI `AppClient` 的型別組合（`BackupClient` 與 `GuardClient`）、開發指南的 crate 表與情境測試清單。本次只整合、不新增行為。
- 理由：已進入整合分支的條目編號不可改動，順延尚未合併的本票（與 AI-165、AI-175、AI-179 同）。
- 替代方案：無（編號規則已定）。
- 影響：無資料庫影響。
- 審閱：未審

### AI-214：票 38 整合 rebuild/v1（票 14）：migration 順延為第 20 版、AI 編號再順延為 AI-208～AI-213
- 重要度：中
- 時間／階段：2026-10-22，票 38 整合 run R078-38-integrate
- 決策：(1) 整合分支已併入票 14，第 19 個 migration 是 `model_overrides_and_native_session`，本票的 `guard` 順延為第 20 個，排在最後；兩者沒有共用資料表（票 14 只對 `workspace`、`run` 加欄，本票新建 `guard`、`lesson_recurrence`），都會套用；開發指南的「migration 19 `guard`」同步改為 20。(2) 整合分支已用到 AI-207（票 14 的 AI-196～AI-207），本票條目 AI-196～AI-201 順延為 AI-208～AI-213，移到 AI-207 之後；條目內引用本票編號之處（AI-213 的「順延為 AI-208～AI-212」）與 AI-212 的 migration 序號（第 20 個）一併更新，因此 AI-213 內文寫的是現行編號，當時實際使用的是 AI-196～AI-200；AI-213 的「`guard` 維持第 19 版」是當時的事實，保留原文。依驗證 R075 的意見補上 AI-212、AI-213 缺的 `- 審閱：未審` 行與條目間空行；票 14 的條目逐字未動。程式碼與票面沒有引用這些編號。(3) 自動合併的 `run.rs` 已核對：票 14 只在 Run 加上 `native_session_id`、新增 `BindNativeSession` 命令、在派工的啟動內容加上 Worker 角色的模型覆寫；設定快照准入在 API 與 Supervisor 的啟動前（AI-202），不在 `check_dispatch`。Guard 檢查仍在 `check_dispatch` 內，順序維持「授權准入之後、相依檢查之前」；`baton-store` 的 `runs.rs` 同時讀寫 `native_session_id` 並在派工時載入 Guard，`scheduler.rs` 同時有本票的 `guard` 等待類別與票 14 的 `cli_version_unknown`。`Run` 多了 `native_session_id` 欄位，本票 `guard/tests.rs` 建立失敗 Run 的測試輔助函式補上 `native_session_id: None`（與票 14 對其他測試的處理相同）。本次只整合、不新增行為。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票（與 AI-206、AI-207 同）。
- 替代方案：無（編號規則已定）。
- 影響：用本票分支建過的開發資料庫，`guard` 記在第 19 版，與整合後的編號不一致，需要刪除重建。
- 審閱：未審

### AI-215：Task 新增「種類」欄位，預設是研究；開發 Task 必須明確宣告
- 重要度：高
- 時間／階段：2026-10-22，票 20 實作
- 決策：Task 新增 `kind`（`development`／`research`／`document`），建立時決定、之後不能改；建立請求省略時是 `research`。只有 `development` 使用 worktree 與整合佇列，`research`、`document` 使用輸出目錄。種類放在 Task 本身（migration 21 的 `task.kind`，舊資料預設 `research`），不放在會改版的契約裡。
- 理由：票面要求「開發類」與「研究與文件」走不同的目錄，但既有模型沒有區分。預設研究最安全：輸出目錄永遠建得起來、不碰 Git；若預設開發，非 Git Workspace 的既有 Task 會全部無法派工。種類不可改，避免已建立的 worktree／輸出目錄與種類不一致。
- 替代方案：依 Workspace 是不是 Git repo 自動推斷（同一個 Git repo 內的研究 Task 也會被當成開發）；放在契約中隨版本變更（要處理種類變更時已建立的目錄）。
- 影響：Controller 的 `propose_task`（票 17）與 UI 建立 Task 時要指定種類；忘了指定的開發工作會在輸出目錄中執行，不會進入整合佇列。
- 審閱：未審

### AI-216：工作目錄放在 Workspace 目錄內的 `.baton\`，並寫入忽略全部內容的 `.baton\.gitignore`
- 重要度：中
- 時間／階段：2026-10-22，票 20 實作
- 決策：開發 Task 的 worktree 在 `<Workspace>\.baton\worktrees\<Task>`（平台分支 `baton/<Task>`），研究與文件 Task 的輸出目錄在 `<Workspace>\.baton\outputs\<Task>`，整合目錄在 `<Workspace>\.baton\integration`。第一次準備時寫入 `.baton\.gitignore`（內容 `*`），讓主 checkout 不出現未追蹤的檔案；不修改 repo 的 `.gitignore` 或 `.git\info\exclude`。
- 理由：Workspace 授權的目標是 Workspace 目錄，權限提示（AI-138）以它判定檔案路徑；放在 `%LOCALAPPDATA%\Baton` 會讓 Worker 的每一次編輯都落在授權範圍外而建立裁定。Claude Code 自己的 `--worktree` 也是放在 repo 內的 `.claude\worktrees`。
- 替代方案：放在 `%LOCALAPPDATA%\Baton\work\…`（需要同時擴充授權目標與權限判定）；放在 Workspace 的同層目錄（授權範圍外，且可能沒有寫入權）。
- 影響：權限判定仍以 Workspace 目錄為基準，相對路徑以 Workspace 目錄解讀而不是 Run 的工作目錄（Write／Edit 工具用絕對路徑，不受影響）。研究 Task 的成果檔案位於 `.baton\outputs\…`，使用者在 Workspace 目錄中看得到。
- 審閱：未審

### AI-217：目錄在派工之前準備；不可用時不派工、排隊並顯示原因，每 60 秒自動重試
- 重要度：高
- 時間／階段：2026-10-22，票 20 實作
- 決策：使用者要求派工時與 Supervisor 排程前，對排隊中的 `ready` Task 在交易外建立目錄，成功或失敗都登記到 `task_workdir`。派工判定只認已登記且可用的目錄；不可用時 Task 留在派工佇列，等待原因類別是新增的 `workdir`，訊息就是登記的失敗原因（不是 Git repo、detached HEAD、建立失敗的錯誤）。失敗後排程每 60 秒重試一次，使用者再次要求派工時立即重試。準備以行程內的一把鎖序列化，同一個 Task 同時被要求派工多次時只建立一次。完全退出中或等待「繼續工作」的 Workspace（票 25 的派工阻擋）不準備目錄，不在這時執行 git。
- 理由：票面要求「目錄不可用時不派工並顯示原因」；若在 Supervisor 啟動程序時才建立，失敗會變成一個已建立又失敗的 Run，與「不派工」不符。建立目錄是外部副作用，不能放在派工交易裡。
- 替代方案：啟動程序前才建立，失敗時以 `launch_failed` 結束 Run；每次 tick 都重試（Git 指令每秒執行一次）。
- 影響：派工 API 在回應之前可能要等 `git worktree add`；已可用的目錄不會重建（目錄被使用者手動刪掉的情況交給票 21 的核對）。
- 審閱：未審

### AI-218：「與 Session／Run 關聯正確」的解讀：一個 Task 一個目錄、跨 Run 沿用，Run 經啟動資訊關聯到目錄
- 重要度：中
- 時間／階段：2026-10-22，票 20 實作
- 決策：同一個 Task 的每個 Run 都在同一個已登記的目錄中執行（第一個 Run 失敗後重新派工也回到同一個 worktree）；不同 Task 的目錄一定不同（可用路徑有唯一索引）。Run 的工作目錄記在派工時固定的啟動資訊中，工作面板的 `workdirs[].run_ids` 由它組出。目前程式中還沒有 Session 實體（票 15），所以 Session 的關聯以「同一個工作目錄」保證：Claude Code 以工作目錄區分專案，換目錄就接不回原生 Session。
- 理由：讓接手的 Run 看到前一個 Run 留下的變更與原生 Session，也不需要在 Run 資料表加欄位。
- 替代方案：每個 Run 一個新的 worktree（並行衝突更少，但接不回 Session、前一個 Run 的成果要另外搬）；在 `run` 加 `workdir` 欄位。
- 影響：票 15 實作 Session 時，Session 應記錄並核對它所在的工作目錄。
- 審閱：未審

### AI-219：開發 Task 通過驗證後停在 `verifying` 排入整合佇列；不新增 Task 狀態，未通過整合回到 `ready`
- 重要度：高
- 時間／階段：2026-10-22，票 20 實作
- 決策：開發 Task 的成果接受交易（確定性檢查全部通過或手動接受）通過時，不再直接 `completed`，而是在同一個交易中排入整合佇列，Task 停在 `verifying`；這個 Run 不再被自動檢查，也不能再手動接受一次（409 `acceptance_already_queued`）。整合完成才 `completed`；合併衝突、整合驗證未通過、目標分支在整合期間被更新時，Task 回到 `ready`（與 AI-151 相同：可修正、不自動重派），原因與證據保存在整合項目。研究與文件 Task 不受影響，仍在驗證通過時完成。
- 理由：票面要求「未通過整合不能宣告完成」。新增 `integrating` 狀態要重建 `task` 資料表的 CHECK 約束並影響所有讀取 Task 狀態的地方；以整合項目的狀態表達進度即可。
- 替代方案：新增 Task 狀態 `integrating`；整合失敗時讓 Task 進入 `blocked` 並建立裁定。
- 影響：UI 看到開發 Task 在 `verifying` 時，要再看整合項目才知道是「等驗證」還是「等整合」（Web UI 已顯示「成果已通過驗證，等待整合」）。
- 審閱：未審

### AI-220：整合在平台專用的整合目錄中合併與驗證，通過才 fast-forward 目標分支；整合目標是建立 worktree 時 Workspace 簽出的分支
- 重要度：高
- 時間／階段：2026-10-22，票 20 實作
- 決策：每個項目在 `<Workspace>\.baton\integration`（detached HEAD）簽出目標分支目前的 commit，以 `--no-ff` 合併平台分支、在那裡執行整合驗證；通過後才把目標分支從合併當時的 commit 以 fast-forward 前移到合併結果（有工作目錄簽出目標分支時在那裡 `merge --ff-only`，否則 `update-ref` 比對舊值）。整合目標分支是建立 worktree 當時 Workspace 簽出的分支；Workspace 在 detached HEAD 時不能準備開發 Task 的 worktree。
- 理由：「合併後執行整合驗證」若直接在主線合併，驗證失敗時主線已經被弄壞，還要 revert；在整合目錄中合併，主線只會前移到驗證過的結果。
- 替代方案：直接在主 checkout 合併再驗證，失敗時 revert；讓使用者在 Workspace 設定整合目標分支（目前沒有設定頁）。
- 影響：主 checkout 有未提交的變更擋住 fast-forward 時，整合以「無法前移」失敗，Task 回到 `ready`。平台不 push 遠端，也不刪除平台分支。
- 審閱：未審

### AI-221：整合驗證執行 Task 契約的同一份確定性檢查；沒有宣告檢查時，乾淨的合併就是整合的證據
- 重要度：中
- 時間／階段：2026-10-22，票 20 實作
- 決策：整合驗證在整合目錄中執行 Task 目前契約宣告的確定性檢查（命令、檔案與成果檢查，與成果驗證同一個執行器），結果依序保存在整合項目的 `checks`。契約沒有宣告檢查（只能手動接受）的開發 Task，合併沒有衝突即視為通過整合驗證。
- 理由：目前沒有 Workspace 層級的「整合驗證設定」，Verification Profile 屬於第四階段（票 45）；同一份檢查最能回答「合併別人的變更後，這個 Task 的完成條件是否仍成立」。
- 替代方案：Workspace 設定一組整合命令；沒有檢查時拒絕整合、要求使用者確認。
- 影響：只宣告了成果檢查的 Task，整合驗證不會實際建置或測試合併結果。之後有 Verification Profile 時，可改為執行 Profile 的整合步驟。
- 審閱：未審

### AI-222：單一整合佇列是全域的，一次處理一個；與驗證在同一個背景工作中依序執行
- 重要度：中
- 時間／階段：2026-10-22，票 20 實作
- 決策：全部 Workspace 共用一個佇列，依排入順序處理；任何時刻最多一個項目在 `merging`／`verifying`／`advancing`（資料表的部分唯一索引也保證）。Supervisor 在確定性檢查之後、同一個背景工作中處理整合，直到沒有可處理的項目。崩潰後停在合併或整合驗證的項目從合併重做（整合目錄整個重設），停在前移的項目以同一個合併結果重試前移（冪等）。前移時發現目標分支已被更新就失敗，不自動重新排入。
- 理由：票面寫「單一整合佇列」；一次一個能保證後一個項目在前一個前移後的主線上合併。不同 repo 之間並行的好處在第一版不明顯。
- 替代方案：每個 Workspace 一個佇列（不同 repo 可並行）；目標分支被更新時自動重新排入。
- 影響：一個 Workspace 的長時間整合驗證（命令最多 10 分鐘）會讓其他 Workspace 的整合等待；與成果驗證共用同一個背景工作，也會延後其他 Task 的驗證。
- 審閱：未審

### AI-223：正式版 Git 操作的細節（作者資訊、hook、整合目錄的防呆）
- 重要度：低
- 時間／階段：2026-10-22，票 20 實作
- 決策：以 `git -C <目錄>` 執行、不經過 shell、移除 `GIT_DIR` 等會讓 `-C` 失效的環境變數。合併 commit 預設用使用者自己的 Git 身分，只有 `user.name`／`user.email` 沒有設定時才以 `Baton <baton@localhost>` 補上。不跳過 repo 的 hook；hook 拒絕合併時放棄並回報錯誤。整合目錄若是殘留的普通目錄、主 checkout 或簽出分支的 worktree，拒絕重設並回報錯誤，不刪除任何檔案。建立 worktree、整合合併與前移之前先 `git worktree prune`，讓崩潰後能在同一路徑重建。分支已經包含在目標中時，合併結果就是目標分支目前的 commit。
- 理由：避免誤刪使用者或 Task 的工作；尊重 repo 的規範。
- 替代方案：一律以 Baton 身分 commit；以 `--no-verify` 跳過 hook。
- 影響：缺少作者設定時的補值路徑沒有自動化測試（需要改行程的環境變數）。
- 審閱：未審

### AI-224：Web UI 在等待整合時隱藏「接受成果」，整合佇列區塊只列這個 Workspace 的項目
- 重要度：低
- 時間／階段：2026-10-22，票 20 實作
- 決策：開發 Task 在 `verifying` 且最新整合項目尚未結束時，卡片顯示「成果已通過驗證，等待整合」，隱藏成果驗證面板與「接受成果」按鈕。工作面板的「整合佇列」區塊只列這個 Workspace 尚未結束的項目並標出處理中的那一個；處理中的項目若在別的 Workspace，這裡不會有任何項目標「處理中」。建立 Task 的表單一律送出 `kind`（預設研究）。
- 理由：排入整合後再接受一定被拒絕；工作面板是以 Workspace 為範圍的畫面。
- 替代方案：工作面板另外讀取全域佇列（`GET /api/v1/integration-queue`，需要新增 Tauri 指令）。
- 影響：使用者要看全域佇列時，目前只有 API 可用。
- 審閱：未審

### AI-225：票 20 整合 rebuild/v1（票 37、25、23、40）：migration 第 19 版、AI 編號順延為 AI-215～AI-224；還原後的整合項目
- 重要度：中
- 時間／階段：2026-10-22，票 20 實作
- 決策：實作期間整合分支合併了票 37（migration 17 `knowledge`）、票 25（migration 18 `lifecycle`）、票 23 與票 40（沒有新增 migration），本票的 `task_workdir_and_integration_queue` 是第 19 個 migration；本票的條目原本編為 AI-159～AI-168，兩次整合後順延為 AI-215～AI-224，內容不變（只更新其中的 migration 編號）。衝突合併方式：派工錯誤與等待原因兩邊都保留（`launch_halted`／`lifecycle` 與 `workdir_*`／`workdir`）；完全退出中或等待「繼續工作」的 Workspace 不準備工作目錄；開發指南、GLOSSARY、Domain 模組說明兩邊都保留。備份（票 40）整份複製控制資料庫，新的兩張表自然包含在內。
- 理由：整合分支的條目與 migration 編號不能變動；本票尚未發布。
- 替代方案：把本票的 migration 插在前面（會改動整合分支已存在的版本號）。
- 影響：以本票分支（合併前）建立過的開發資料庫需刪除重建。還原（票 40）時若備份當下有處理中的整合項目，還原後 Supervisor 會從合併重做並可能前移目標分支（Git 的實際狀態以目前 repo 為準，前移時目標分支已移動就以失敗結束），沒有像 Run 一樣在還原時一律結束；需要時由票 40／31 的恢復規則補上。
- 審閱：未審

### AI-226：票 20 整合 rebuild/v1（票 14、38）：migration 順延為第 21 版、AI 編號再順延為 AI-215～AI-225
- 重要度：中
- 時間／階段：2026-10-22，票 20 整合 run R080-20-integrate
- 決策：(1) 整合分支已併入票 14（第 19 個 migration `model_overrides_and_native_session`，AI-196～AI-207）與票 38（第 20 個 migration `guard`，AI-208～AI-214）。本票的 `task_workdir_and_integration_queue` 順延為第 21 個，排在最後；三者沒有衝突的資料表（票 14 對 `workspace`、`run` 加欄，票 38 新建 `guard`、`lesson_recurrence`，本票對 `task` 加 `kind` 並新建 `task_workdir`、`integration_entry`），都會套用。migration 註解、開發指南的「migration 19」與 AI-215 內的 migration 序號同步改為 21。(2) 本票條目 AI-196～AI-206 順延為 AI-215～AI-225，移到 AI-214 之後；條目內引用本票編號之處（AI-225 的「順延為 AI-196～AI-205」）一併改為現行編號；AI-225 的「migration 第 19 版」是當時的事實，保留原文（與 AI-214 對 AI-213 的處理相同）。本票原本缺審閱行的 AI-224、AI-225 補上 `- 審閱：未審`。整合分支的條目逐字未動；程式碼、文件與票面沒有引用這些編號。(3) 衝突合併方式：`RunContext` 同時帶本票的 `workdirs` 與票 38 的 `guards`；派工時 `dispatch` 重新接收 `Workspace`，啟動內容的工作目錄用本票登記的工作目錄、模型用票 14 的 Worker 角色覆寫；`check_dispatch` 的順序是授權准入、Guard、相依、工作目錄；等待原因同時有 `guard` 與 `workdir` 類別（Rust 的 `scheduler.rs` 與 Web 的 `WaitCategory`）；Store 的 `Loaded` 同時有 `workdirs`、`integrations` 與 `guards`；桌面殼的匯入、production_ports 的模組說明、開發指南的 crate 表、章節與情境測試清單兩邊都保留。票 38 的 `guard/tests.rs` 建立 Task 的測試輔助函式補上 `kind: TaskKind::default()`（本票為 `Task` 新增的欄位；與本票對其他測試的處理相同，不改變測試內容）。本次只整合、不新增行為。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票（與 AI-214、AI-225 同）。
- 替代方案：無（編號規則已定）。
- 影響：用本票分支建過的開發資料庫，`task_workdir_and_integration_queue` 記在第 19 版，與整合後的編號不一致，需要刪除重建。本票較早的 commit 訊息與 `.orchestration` 下的驗證紀錄（R076）提到的 AI-196～AI-206 是改號前的編號。
- 審閱：未審

### AI-227：Controller 動作以 `controller_action` 工具「事前判定」；必須送裁定的動作被拒絕並轉成單次核准的裁定請求
- 重要度：高
- 時間／階段：2026-10-22，票 35 實作
- 決策：(1) 決策權限表（`baton_domain::authority`）把 spec D-15 列成 17 個事項：7 個可自主處理（拆分合併排序重試 Task、選 Playbook、派研究 Worker、把 finding 分類為修正、預算內重派、建立調查 Task、提交候選教訓）、9 個必須送裁定（產品方向與範圍取捨、修改完成條件或驗收標準、超出授權、外部寫入／部署／發訊／付費／刪除正式資料、駁回 finding、跳過 Stage 或 Gate、結果未知外部操作的處置、接受設計偏離、預算用盡後的下一步）、1 個只能人工發起（三次恢復失敗後的重試）。(2) 新增 Agent 工具 `controller_action {matter, task_id?, summary, reason, evidence?, urgency?}`：**工具本身不執行任何動作**，只依 Workspace 目前生效的權限表判定並回 `allowed`／`requires_decision`／`approved`／`denied`／`human_only`。必須送裁定時建立 `kind: "authority"` 的裁定請求（選項 `approve`「核准這一次」、`reject`「不核准」，阻擋目標 Task；目標 Task 是 `running` 時轉 `waiting_decision`，與 AI-140 相同）；同一 Task 同一事項仍待處理時回既有請求。使用者以「接受」或「修改條件」選 `approve` 才算核准，下一次同一 Task 同一事項的呼叫消耗一次（`approved`，修改條件時帶 `conditions`）；其他處理方式（駁回、選 `reject`、回答、要求調查）一律在下一次呼叫回 `denied` 並消耗。只能人工發起的事項回 `human_only`，不建立請求。自主與人工發起的判定各追加一筆 `authority_audit` 稽核事件。(3) 目前沒有 Controller 身分（票 16、17 未做），比照 AI-161 暫時開放給所有 Agent 憑證。
- 理由：驗收條件要求「Controller 試圖修改完成條件或跳過 Gate 被拒絕並轉為裁定請求」，但 Controller 的變更工具屬票 16、17，現在 Agent 根本沒有改契約或跳過 Gate 的工具；做成通用的事前判定工具，票 17 的每個變更工具都能共用同一個判定與單次核准，不必重做。核准沿用權限提示的「單次、消耗一次」模型，維持「Agent 不能擴大授權」。
- 替代方案：現在就做 `update_task_contract`、`skip_gate` 等具體的 Agent 變更工具並在其中判定（等於提前實作票 17，且要決定核准後由誰套用變更）；核准後永久有效（等於偷偷放寬權限表）；只能人工發起的事項也建立請求（違反 spec「只能人工從恢復介面發起」）。
- 影響：`matter` 是 Agent 自報的，自報「可自主處理」的事項就會得到 `allowed`；這只是判定，真正的變更仍要經過各自的工具。**票 16、17 實作 Controller 變更工具時，必須由 Runtime 依實際變更推導事項（例如契約的 `done_criteria`／`verification_checks` 改變 → `modify_done_criteria`）再呼叫 `authority` 判定，不能信任自報的 `matter`**，並在 Controller 有專屬憑證後把 `controller_action` 收回為 Controller 專屬。
- 審閱：未審

### AI-228：Workspace 收窄權限表的形狀：取代式設定、revision 校驗、只記被收窄的事項
- 重要度：中
- 時間／階段：2026-10-22，票 35 實作
- 決策：`GET／PUT /api/v1/workspaces/{id}/decision-authority`。`PUT {expected_revision, entries: [{matter, effective}]}` 是取代式（沒列出的事項回到內建值），`revision` 從 0 起、每次成功加一，不符回 409 `authority_revision_mismatch`。任何一項比內建寬（例如把「修改完成條件」設為可自主），或調整「只能人工發起」的事項，整個請求 400 `authority_widening_refused`、什麼都不寫。可自主處理的事項設為「必須送裁定」或「只能人工發起」都記成「收窄為必須送裁定」。資料存在 `decision_authority_policy`（每個 Workspace 一列，只記被收窄的事項清單），變更追加 `decision_authority_changed` 事件。收窄不影響已建立的請求與已給出的核准。
- 理由：「可收窄不可放寬」用「只存收窄清單」表達，資料層就不可能存出放寬的設定；取代式讓 UI 一次送出整張表，搭配 revision 避免兩個視窗互相覆蓋。
- 替代方案：逐項 PATCH（多個請求才能完成一次設定，中途失敗會留下半套）；允許 Workspace 把事項設為「只能人工發起」（多一種狀態，目前沒有對應的人工介面）。
- 影響：未知的 `matter` 在 JSON 解析就被拒（400）。權限表目前只約束 `controller_action`，其他既有路徑（例如 Worker 的 `request_decision`）不受影響。
- 審閱：未審

### AI-229：「目標版本」與過期的定義：目標 Task 與契約版本；過期在開啟裁定中心、查詢待裁定數與提交時標記；過期會恢復只被它阻擋的 Task
- 重要度：高
- 時間／階段：2026-10-22，票 35 實作
- 決策：(1) 每個請求記下 `target {task_id, contract_version, authorization_revision}`：Agent 與權限提示的目標是提出者的 Task，Controller 動作的目標是 `task_id` 指定的 Task。目標 Task 的契約版本改變、Task 已結束（completed／cancelled）或不存在時，請求就「目標版本已變更」。(2) 權限提示另外記下判定當時的授權 revision 只供查閱，**授權變更不會讓請求過期**：修改授權與回答請求是兩回事（票 23 的情境測試即如此），單次批准在執行時逐項核對綁定（AI-180～AI-182）。(3) 過期的請求標為 `superseded`（記 `status_reason`、`revision` 加一、事件 `decision_superseded`），不能再套用；只被它阻擋、仍在 `waiting_decision` 的 Task 回到 `ready` 並在同一個交易重新排隊，喚醒 Outbox 帶 `superseded: true`，提出者重新開工後可依新契約再提出。(4) 標記時機：開啟裁定中心（清單／詳情）、查詢待裁定數（狀態列）、提交處理時；**不**在修改契約的交易裡連動。提交時發現過期：過期標記與 Task 恢復照常寫入，API 回 409 `decision_superseded`。
- 理由：使用者故事 54「目標版本已改變的請求被標記為已過期並重新整理」；過期不是同意也不是拒絕，所以不留下任何裁定，只讓工作依新契約重新判斷，否則 Task 會卡在一個永遠不能回答的請求上。延後到開啟或提交時才標記，不必修改其他票的契約更新路徑（避免與同時進行的票衝突），而使用者能看到或套用請求的入口都會先重新整理。
- 替代方案：修改契約時同一交易標記（要改 Task 的寫入路徑，與其他票衝突面大）；過期的請求讓 Task 進入 `blocked`（要使用者再手動處理一次，Agent 也無從得知）；把授權 revision 也算進權限提示的目標（與已驗收的票 23 行為衝突）。
- 影響：契約更新後到有人開啟裁定中心之前，Task 仍停在 `waiting_decision`、Agent 不會被喚醒；Controller 事件迴圈（票 16 之後）若需要即時反應，可在讀取時呼叫同一個重新整理命令（`DecisionCommand::Refresh`）。GET 端點會寫入資料（標記過期）。
- 審閱：未審

### AI-230：`expected_revision` 必填；內容版本在延後、合併、過期、回答時加一，連結不加；已回答的同樣處理重送仍冪等
- 重要度：中
- 時間／階段：2026-10-22，票 35 實作
- 決策：處理請求（單筆與批次）一律帶 `expected_revision`，缺少 400、不符 409 `decision_revision_mismatch` 且不寫入任何東西。`revision` 從 1 起，延後、被合併進來（阻擋範圍變大）、被合併出去、過期、回答時加一；新增連結不加（決定的內容沒有變）。已回答的請求收到「同樣的處理方式與選項」視為冪等 200，不比對 revision；不同的處理 409 `decision_already_answered`。檢查順序：找不到 → 已結束（冪等／已回答／已過期／已合併）→ 目標版本已變更 → revision → 處理方式與欄位驗證。既有的票 22 用戶端與測試改為帶 revision。
- 理由：選擇性的 expected_revision 等於沒有校驗；把阻擋範圍的變動（合併）算進 revision，避免使用者以為只影響一個 Task 卻一次恢復兩個。
- 替代方案：用 Task 契約版本當 expected_revision（無法偵測合併、延後等請求本身的變動）；連結也加一（使用者在看的時候有新相似請求出現，就會莫名其妙提交失敗）。
- 影響：舊版 App 不帶 revision 會被拒；App 與 Runtime 同版發佈，沒有相容問題。
- 審閱：未審

### AI-231：六種處理方式的語意：延後不結束請求；回答與要求調查結束請求並恢復工作；權限提示只有接受、延後、駁回
- 重要度：高
- 時間／階段：2026-10-22，票 35 實作
- 決策：`action` 為 `accept`（選一個選項）、`modify`（選一個選項＋必填修改後的條件）、`answer`（必填回答文字，不選選項）、`investigate`（必填要調查的事）、`defer`（`defer_minutes` 1 分鐘到 30 天，預設 1 天）、`reject`（理由選填）。除了延後，其他五種都寫一筆裁定（`decision.action` 記處理方式，選項只在接受與修改條件時保存）並結束請求、恢復受阻的 Task，喚醒 Outbox 帶 `action`、`option_id`、`note`，由提出者依內容繼續（例如要求調查時先調查再視需要重新提出）。延後**不是裁定**：請求仍待處理、Task 仍等待、只記 `deferred_until` 並把它排在可處理的請求之後，時間到只是恢復顯示順序，不會發生任何狀態變化。權限提示只允許接受、延後、駁回（駁回視同「拒絕」，下一次同一操作的提示回 `denied`），其他三種 400 `decision_action_not_allowed`。
- 理由：使用者故事 51 的六種處理方式；延後必須和「逾時不是同意」（AI-141）一致。權限提示的答案只會被機械地消耗成允許或拒絕，修改條件、回答、要求調查對它沒有可執行的意義。
- 替代方案：要求調查時請求保持待處理（工作卡住，Agent 也收不到要調查什麼）；延後也結束請求（沉默變成某種決定）；權限提示的修改條件視為允許（條件無法被機械核對，等於無條件放行）。
- 影響：Worker／Controller 收到的回答文字只在喚醒 Outbox 中（目前沒有消費者，AI-142）；Context Builder（票 39）載入決策時要讀 `decision.action` 與 `note`。資料庫以 CHECK 保證只有接受與修改條件有選項、修改條件／回答／要求調查一定有文字。
- 審閱：未審

### AI-232：批次處理逐項各自一個交易；`batch_id` 只做稽核；最多 50 項、可跨 Workspace
- 重要度：中
- 時間／階段：2026-10-22，票 35 實作
- 決策：`POST /api/v1/decisions/batch {items}`，每一項與單筆處理相同（含 `workspace_id`、`expected_revision`），依序各自一個交易套用；一項失敗（例如 revision 不符、已過期）不影響其他項，回應逐項列出 `ok` 或錯誤代碼。批次產生一個 `batch_id` 記在該批每一筆裁定上，**只用於稽核**，不存在任何批次層級的授權紀錄。空批次、超過 50 項、同一請求出現兩次，整批 400 `decision_batch_invalid`。任何一項成為正式裁定就要求一次重要裁定備份（票 40，同一小時內合併）。UI 的批次面板只提供逐項選擇，沒有「全部套用同一個答案」。
- 理由：驗收條件「批次裁定每項各自保存，不形成概括授權」；逐項交易讓「一項過期」不會拖垮整批，也讓每一筆的效果與單筆處理完全相同（例如權限提示的單次批准仍逐項綁定、逐項消耗）。
- 替代方案：整批一個交易（一項失敗全部回滾，使用者要重新處理全部）；允許同一個答案套用到多項（容易形成概括授權）。
- 影響：批次不是原子的，部分成功是預期結果，UI 要逐項顯示。
- 審閱：未審

### AI-233：相似問題的判定、自動連結與合併的限制
- 重要度：中
- 時間／階段：2026-10-22，票 35 實作
- 決策：(1) 相似鍵：同種類，Agent 請求比較「問題」正規化後的文字（只保留字母、數字與各種文字，英文轉小寫），權限提示比較操作指紋（工具＋輸入），Controller 動作比較「事項＋摘要」。(2) 新請求建立時，與同 Workspace 相似鍵相同的**待處理**請求自動互相連結（雙向、存在 `decision_request_link`）；使用者也可以手動連結任意兩個請求。連結只是提示，不改變任何行為與 revision。(3) 合併：使用者把 A 合併到 B，A 變成 `merged`（`merged_into = B`），A 的阻擋 Task 加入 B 的阻擋範圍，兩者 revision 各加一並互相連結；之後處理 B 就恢復兩者的 Task，並各自叫醒 A、B 的 Run。只能合併同種類、都待處理的請求，**權限提示不能合併**（單次批准綁定 Task 與操作，合併會讓一個批准用在兩個 Task 上）；處理已合併的 A 回 409 `decision_merged`。
- 理由：使用者故事 53「相似的問題被合併或互相連結」；自動合併會替使用者決定兩個問題相同，所以自動只做連結，合併一定由使用者發起。
- 替代方案：自動合併相似問題（可能誤判，且會擴大阻擋範圍）；以語意相似度判定（需要模型或外部服務，不可重現）。
- 影響：同一個 Run 重複提出完全相同的問題仍回到既有請求（AI-143），不會產生連結。正規化是保守的，措辭不同的同一問題不會被自動連結，要靠使用者手動連結或合併。
- 審閱：未審

### AI-234：migration 22 以「建新表、搬資料、由子到父刪舊表、改名」重建裁定四張表
- 重要度：中
- 時間／階段：2026-10-22，票 35 實作
- 決策：SQLite 不能修改 CHECK 約束，而 `decision_request` 的種類與狀態、`decision` 的 `option_id NOT NULL` 都要放寬，所以 migration 22（`decision_center_full`）重建 `decision_request`、`decision_request_block`、`decision`、`decision_approval_use`：先建 `*_next` 新表並搬資料，再由子到父 `DROP` 舊表，最後改名。這四張表只互相參照、沒有其他表參照它們，所以在 `foreign_keys=ON` 的 migration 交易內可行（實測：直接重建單一父表或改名舊表，外鍵檢查會失敗或參照被改寫）。既有資料：目標 Task 取原 Task、契約版本取該 Task 目前的版本，權限提示的授權 revision 取 Workspace 目前的版本；處理方式一律記為 `accept`；revision 待處理為 1、已回答為 2。新表的觸發器：只有待處理的請求能更新、只能改狀態／revision／延後／合併／原因、revision 只增不減；阻擋範圍與連結不可刪除；裁定與消耗紀錄維持追加式。另建 `decision_request_link` 與 `decision_authority_policy`。
- 理由：保留票 22 的資料與追加式保證，同時讓新的狀態在資料庫層也受約束。
- 替代方案：另開「狀態」側表，讓舊欄位的 `answered` 代表「已結束」（兩個狀態欄位語意不一致）；暫時關閉外鍵（migration 在交易內，無法切換 `foreign_keys`）。
- 影響：之後其他票若新增參照這四張表的外鍵，未來再重建時要一併處理。migration 前備份（票 40）照常涵蓋這次重建。
- 審閱：未審

### AI-235：票 35 整合 rebuild/v1（票 25、23、40、14）：migration 順延為第 20 個、AI 編號順延為 AI-227～AI-235、移植票 23 的綁定判定
- 重要度：低
- 時間／階段：2026-10-22，票 35 實作
- 決策：(1) 票 25 的 `lifecycle` 是第 18 個 migration、票 14 的 `model_overrides_and_native_session` 是第 19 個，本票的 `decision_center_full` 順延為第 20 個；本票條目原本的 AI-196～AI-204 與票 14 的條目撞號，順延為 AI-227～AI-235 並移到 AI-207 之後，條目內與開發指南的引用一併改成新編號。(2) 票 23 改寫的 `permission_prompt`（單次批准綁定、結果未知要先查證、選項文字）移植到本票改寫後的結構：答案是否核准改以「接受且選 `allow_once`」判定，駁回視同拒絕；事件版本改用請求聚合的接續版本而非固定的 3；`DecisionView` 的權限細節保留票 23 的 `binding`、`prior_use`、`rejected_approval`、`approval_use`。(3) 依票 23 已驗收的行為，授權變更不讓權限提示過期（見 AI-229），本票原本的對應情境測試改為驗證「仍可回答」。(4) 票 40 的「正式裁定要求備份」套用到單筆與批次處理，延後不觸發。(5) 票 23、40 的情境測試回答時補上 `expected_revision`。前端衝突以本票的新版面為主體，整合票 23 的單次批准綁定顯示與「修改 Workspace 授權」入口。開發指南中「權限提示的 CLI 相容性待驗證」一句改用票 14 已實測的說明。
- 理由：整合分支上已驗收的行為優先；只做必要的移植，不改變票 23、25、40 的語意。
- 替代方案：保留「授權變更使權限提示過期」並修改票 23 的測試（推翻已驗收的行為）。
- 影響：本票沒有新增其他行為。本票較早的 commit 訊息（含第一次合併 commit）提到的 AI-196～AI-204、migration 19 是改號前的編號。
- 審閱：未審

### AI-236：票 35 整合 rebuild/v1（票 38）：migration 再順延為第 21 個、AI 編號再順延為 AI-227～AI-235
- 重要度：中
- 時間／階段：2026-10-22，票 35 整合 run R082-35-integrate
- 決策：(1) 整合分支已併入票 38，第 20 個 migration 是 `guard`（新建 `guard`、`lesson_recurrence`），本票的 `decision_center_full` 順延為第 21 個，排在最後；兩者沒有共用資料表（`guard` 只參照 `knowledge_entry`、`workspace`、`task`，不參照本票重建的四張裁定表），都會套用。開發指南「migration 20 重建」與 AI-234 的 migration 序號同步改為 21。(2) 整合分支已用到 AI-214（票 38 的 AI-208～AI-214），本票條目 AI-208～AI-216 順延為 AI-227～AI-235，移到 AI-214 之後；條目內引用本票編號之處（AI-235 的「順延為 AI-227～AI-235」「見 AI-229」）與開發指南裁定中心段落的引用一併更新，因此 AI-235 內文寫的是現行編號，當時實際使用的是 AI-208～AI-216；AI-235 的「migration 順延為第 20 個」是當時的事實，保留原文。補上 AI-235 原本缺的 `- 審閱：未審` 行。票 38 的條目逐字未動。(3) 衝突處兩邊都保留：`baton-store` 的匯出（本票的 `DecisionOverview` 與票 38 的 `GuardApplyError`、`GuardOverview`）、開發指南的 crate 表（本票的決策權限表、裁定 API 與票 38 的 Guard 模組、`/api/v1/guards`）與情境測試清單（`decision_center_full.rs` 與 `guards.rs`）。本次只整合、不新增行為。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票（與 AI-214、AI-235 同）。
- 替代方案：無（編號規則已定）。
- 影響：用本票分支建過的開發資料庫，`decision_center_full` 記在第 20 版，與整合後的編號不一致，需要刪除重建。本票較早的 commit 訊息提到的 AI-208～AI-216、migration 20 是本次改號前的編號。
- 審閱：未審

### AI-237：票 35 整合 rebuild/v1（票 20）：migration 再順延為第 22 個、AI 編號再順延為 AI-227～AI-236
- 重要度：中
- 時間／階段：2026-10-22，票 35 整合 run R084-35-integrate
- 決策：(1) 整合分支已併入票 20，第 21 個 migration 是 `task_workdir_and_integration_queue`（對 `task` 加 `kind`，新建 `task_workdir`、`integration_entry`），本票的 `decision_center_full` 順延為第 22 個，排在最後；本票重建的四張裁定表只互相參照，票 20 的新表與新欄位都不參照它們，重建時的 `DROP TABLE` 不受影響，兩者都會套用。migration 註解、開發指南「migration 21 重建」與 AI-234 的 migration 序號同步改為 22。(2) 整合分支已用到 AI-226（票 20 的 AI-215～AI-226），本票條目 AI-215～AI-224 順延為 AI-227～AI-236，移到 AI-226 之後；條目內引用本票現行編號之處（AI-235、AI-236 的「順延為 AI-227～AI-235」、AI-235 的「見 AI-229」、AI-236 的「AI-234 的 migration 序號」等）與開發指南裁定中心段落的引用一併更新，因此 AI-235、AI-236 內文寫的是現行編號；AI-236 中「AI-208～AI-216」是那次改號前的舊編號、「第 21 個」是當時的事實，保留原文。補上 AI-236 原本缺的 `- 審閱：未審` 行與它前面的空行。票 20 的條目逐字未動。(3) 衝突處兩邊都保留：`baton-store` 的 migration 清單（票 20 在前、本票在後）、開發指南的 Domain 模組說明（本票的決策權限表與票 20 的 `workdir.rs`、`integration.rs`）與情境測試清單（`decision_center_full.rs` 與 `worktrees.rs`）。本票 `decision/tests.rs` 建立 Task 的測試輔助函式補上 `kind: TaskKind::default()`（票 20 為 `Task` 新增的欄位；與票 20 對其他測試的處理相同，不改變測試內容）。本次只整合、不新增行為。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票（與 AI-226、AI-236 同）。
- 替代方案：無（編號規則已定）。
- 影響：用本票分支建過的開發資料庫，`decision_center_full` 記在第 21 版，與整合後的編號不一致，需要刪除重建。本票較早的 commit 訊息與 `.orchestration` 下的紀錄提到的 AI-215～AI-224、migration 21 是本次改號前的編號。
- 審閱：未審

### AI-238：「准入」就是 Supervisor 持久化 launch intent 的交易；派工時記下的 revision 不算准入
- 重要度：高
- 時間／階段：2026-10-22，票 24 實作
- 決策：R1-a 的「已持久化准入」定義為 `RunCommand::BeginLaunch`（launch intent 與 command_id 寫入、`queued` → `starting`）提交的那一刻。這個交易以**當下生效**的授權重新檢查 Task 目前契約需要的能力（`authorization::admit`），通過就把 Run 的 `authorization_revision` 改寫為當下的 revision，並記下新欄位 `admitted_at` 與 `settings_version`（migration 21 `run_admission`）。派工（`Dispatch`）仍照舊記下派工當時的 revision，但那只是排隊中的紀錄；排隊中、已有派工 Outbox 但尚未 `BeginLaunch` 的 Run 都算未准入。授權更新與 `BeginLaunch` 都經過 Store 的單一寫入佇列，兩者的先後以事件表的 `seq`（`authorization_confirmed` 對 `run_launch_started`／`run_admission_denied`）查驗。票 06 的 `check_admission` 依 AI-53 的要求改走寫入佇列（不寫任何東西），結果與授權更新依同一個順序排列。
- 理由：票面明說「未准入者（即使已排隊或有 Outbox）用新版重新檢查」，所以派工不能是准入；AI-87 已把 Agent 憑證的簽發放在 `BeginLaunch` 之後、spawn 之前，准入放在同一點，程序一出生拿到的憑證與授權版本就一致。
- 替代方案：派工即准入（排隊中的 Run 會沿用撤權前的授權，違反票面）；spawn 成功、回執寫回時才算准入（程序已經在跑，撤權時無法阻止它啟動）。
- 影響：Run 多了 `admitted_at`、`settings_version` 兩欄（API 與 Web 型別同步）；票 24 之前已在執行的 Run 這兩欄是 NULL，沿用派工時記下的 revision。交接 successor（尚未實作）也必須經過同一個 `BeginLaunch` 准入，不能複製前任的 revision。
- 審閱：未審

### AI-239：准入被拒時 Run 以 interrupted 結束、Task 回 ready 並自動重新排隊
- 重要度：中
- 時間／階段：2026-10-22，票 24 實作
- 決策：`BeginLaunch` 准入被拒時不寫 launch intent、不簽發憑證、不啟動程序；Run 從 `queued` 直接進入 `interrupted`（`RunStatus::allowed_next` 新增 `Queued → Interrupted`），`end_reason` 為 `admission_denied：<原因>`，事件 `run_admission_denied`（payload 帶 `code` 與當下的 `authorization_revision`），釋放寫入所有權、派工 Outbox 標為處理完畢；Task 只在仍是 `running` 時回到 `ready`，並在**同一個交易**中重新放進派工佇列（優先序 0）。排程照常判定，等待原因以「授權」類別顯示；使用者恢復授權後自動派出。Supervisor 看到 `BeginLaunch` 的結果不是 `starting` 就不 spawn。
- 理由：票面要求未准入者「用新版重新檢查」，被拒的工作不是失敗，也不是使用者取消；放回佇列才能在面板上以既有的等待原因呈現「為什麼還沒跑」，與 AI-168（完全退出停下的 Run 重新排隊）一致。留在 `queued` 並每秒重試（像 `launch_halted` 那樣）會讓 Task 一直顯示「執行中」卻沒有程序，也看不到原因。
- 替代方案：以 `cancelled` 結束（語意是使用者取消）；以 `failed` 結束且不重新排隊（AI-96 的失敗語意，使用者要手動重派）；Run 留在 `queued` 等授權恢復。
- 影響：重新排隊不保留原本的優先序。Run 的狀態表多一條 `queued → interrupted`。
- 審閱：未審

### AI-240：已准入 Run 的權限提示依它准入時的 revision 判定（撤權與擴權都不影響它）
- 重要度：高
- 時間／階段：2026-10-22，票 24 實作
- 決策：`permission_prompt` 改為以 Run 的 `authorization_revision`（准入時固定的版本）判定，不再看 Workspace 目前生效的 revision（`DecisionContext` 新增全部 revision）。所以撤權後已准入的 Run 仍會自動允許舊範圍內的操作；之後**擴大**授權也不會套用到已准入的 Run（它的提示照舊建立裁定請求）；准入時還沒有任何授權的 Run（契約不需要能力）之後即使確認了授權，它的提示一律範圍外。修改授權的影響說明同步改寫（原本寫「執行中的 Worker 下一次權限提示就依新授權判定」，與 R1-a 衝突）。票 22 的兩個情境測試原本先啟動 Run 再確認授權、期待自動允許，改為先確認授權再派工（新增 `two_running_tasks_under_dev_template`）。
- 理由：ADR-0003 的 R1-a 寫明「撤銷前已准入的 Run 沿用舊授權與設定版本」「已准入 Run 的撤權沒有固定完成時間，使用者可另行暫停或取消」；權限提示正是授權實際生效的地方，若仍看目前的 revision，撤權就會立即作用在執行中的 Run，與 ADR 與票面的「沿用舊版本」矛盾，也讓「仍有 N 個 Run 沿用舊授權」失去意義。擴權不套用到已准入 Run 是同一條規則的另一面，且較保守。
- 替代方案：撤權立即生效、擴權也立即生效（AI-183 原本的說法；違反 R1-a）；取兩個 revision 中較嚴格者（撤權立即生效，但不符合 ADR 已確認的取捨）。
- 影響：使用者撤權後若要立刻止損，必須用授權畫面的暫停／取消入口；擴權後要讓執行中的工作受益，得等它的下一個 Run。AI-183 的「影響」中「執行中的 Worker 下一次權限提示就依新授權判定」不再成立。
- 審閱：未審

### AI-241：「沿用舊授權」的 Run 清單、暫停與取消入口的語意
- 重要度：中
- 時間／階段：2026-10-22，票 24 實作
- 決策：(1) `GET …/authorization` 新增 `legacy_runs`：狀態為 `starting`／`running`、沿用的 revision 比目前舊、而且舊 revision 有目前已撤銷或縮小的能力（`authorization::revoked_capabilities`：整個移除、移除目標或增加限制；新增的能力不算）的 Run，附 Task 標題、revision、准入時間、`cancel_requested` 與 `revoked_capabilities`。只有擴權時不列入。授權畫面在清單非空時顯示「後續執行已停用，仍有 N 個 Run 沿用舊授權」。(2) 「暫停」= 既有的取消 Run（`POST …/runs/{id}/cancel`）：停止程序、Task 回到可派工但不自動重派，之後重新派工時依新授權准入；「取消」= 既有的取消 Task。沒有新增「暫停 Run」的狀態或端點。(3) 工作面板 `GET …/tasks` 新增 `authorization_revision`（目前生效的），Run 卡片在 Run 尚未結束、已准入且沿用的 revision 較舊時標示「沿用舊授權（revision X，目前 Y）」（擴權後的舊 Run 也會標示）。
- 理由：票面只要求「相關 Run 的暫停／取消入口」；系統目前沒有 Run 層級的暫停，而「停止這個 Run、保留 Task 等之後依新授權再派」正是撤權情境下使用者要的暫停，重用既有端點不需要新的狀態機。清單只列保有已撤銷範圍的 Run，標題「後續執行已停用」才正確。
- 替代方案：新增 Run 的暫停狀態（狀態機與 Supervisor 都要改）；「暫停」改為暫停整個 Workspace（影響其他 Task）；清單列出所有沿用舊 revision 的 Run（擴權時標題不成立）。
- 影響：「暫停」後 Task 回到 ready 但不會自動重新排隊（沿用取消 Run 的既有行為，AI-66）。
- 審閱：未審

### AI-242：共同設定版本也在准入時固定；已准入 Run 的快照只能用那個版本
- 重要度：中
- 時間／階段：2026-10-22，票 24 實作
- 決策：`BeginLaunch` 在同一個交易中記下 `settings_version`：Run 已有快照綁定（AI-113 的 `run_settings_snapshot`）就用綁定的版本，否則用最新採用的版本（沒有採用任何版本為 NULL）。`POST …/runs/{id}/settings-snapshot` 對已准入的 Run：省略版本時用 `settings_version`、指定其他版本回 409 `settings_snapshot_pinned`；准入時沒有任何版本的 Run 回 409 `shared_settings_not_adopted`（之後採用的版本不會套用到它）。尚未准入的 Run 維持原本的行為（預設最新、可指定）。准入交易不寫快照目錄，也不建立綁定列，因為目錄要在交易外寫檔；票 14 的 Supervisor 在准入之後以 `RunSettings::for_launch` 產生快照，整合後改為使用 Run 的 `settings_version`（不是當下最新版本），准入時沒有任何版本就不帶共同設定啟動。
- 理由：票面要求「沿用原授權與設定版本」；把版本號固定在准入交易中，才能與授權得到同一個持久化順序。綁定表需要目錄路徑與內容雜湊，與交易外的檔案寫入耦合，所以只記版本號。
- 替代方案：准入時直接寫快照目錄並綁定（交易中做檔案 I/O，或交易外寫檔後再補綁定，兩段之間崩潰會留下不一致）；不處理設定版本（只做到授權）。
- 影響：`BeginLaunch` 與 `for_launch` 之間採用的新版本不會被這個 Run 拿到（票 14 原本取「當下最新」，兩者之間有空窗）。AI-202 的「尚未綁定就以最新版本建立」對 Supervisor 而言改為「以准入時固定的版本建立」。
- 審閱：未審

### AI-243：票 24 整合 rebuild/v1（票 40、14）：AI 編號順延為 AI-208～AI-212、migration 第 20 版、快照改用准入時的設定版本
- 重要度：低
- 時間／階段：2026-10-22，票 24 實作（run R071-24-impl）結束前整合
- 決策：(1) rebuild/v1 在本票實作期間併入票 40（AI-186～AI-195）與票 14（AI-196～AI-207，migration 19 `model_overrides_and_native_session`），本票的 AI-196～AI-200 順延為 AI-208～AI-212，移到 AI-207 之後，內容除下述外不變；本票的 migration `run_admission` 順延為第 20 版，`run` 表欄位順序為 `…, profile_id, native_session_id, admitted_at, settings_version`。(2) 票 14 的 `RunSettings::for_launch` 改為接收 Run 准入時固定的 `settings_version`（Supervisor 從 `BeginLaunch` 的結果取得），`materialize` 加上 `admitted` 參數，沿用本票對已准入 Run 的版本規則（AI-242）。(3) 本票情境測試中「快照端點第一次回 201」改為 200：票 14 的 Supervisor 啟動時已建立快照。(4) 程式與文件沒有引用本票的 AI 編號，開發指南的 migration 編號同步改為 20。
- 理由：整合分支既有的 AI 條目與 migration 編號不可改動，順延尚未合併的本票。准入與快照必須使用同一個版本，R1-a 的「沿用原設定版本」才成立。
- 替代方案：保留原編號（與票 14 重號）；`for_launch` 維持取最新版本（准入與快照之間採用的新版本會被已准入 Run 拿到）。
- 影響：無其他行為變更。
- 審閱：未審

### AI-244：票 24 第二次整合 rebuild/v1（票 38）：AI 編號再順延為 AI-215～AI-220、migration 第 21 版、准入同時有 Guard 與授權
- 重要度：低
- 時間／階段：2026-10-22，票 24 實作（run R071-24-impl）結束前整合
- 決策：(1) rebuild/v1 再併入票 38（AI-208～AI-214，migration 20 `guard`），本票的 AI-208～AI-213 順延為 AI-215～AI-220，移到 AI-214 之後；AI-219、AI-220 內的 migration 與交互引用改為新編號，AI-220 記錄的歷史編號保留原文。本票的 migration `run_admission` 順延為第 21 版。自動合併時 AI-220（原 AI-213）的 `- 審閱：未審` 行與票 38 的 AI-208 標題相鄰而被吃掉，重建時補回。(2) `RunContext` 同時帶票 38 的 `guards`（派工時載入）與本票的 `settings_version`（`BeginLaunch` 時取得）；Store 的 `apply_run_command` 依命令分別載入。(3) 開發指南的情境測試清單兩邊都保留（`guards.rs`、`authorization_revocation.rs`）。
- 理由：整合分支既有的編號不可改動。票 38 的 Guard 在派工交易檢查（AI-208），本票的授權在 launch intent 交易重新檢查（AI-238）：兩者的「准入」指不同的交易，但規則互不干擾——派工時兩者都檢查，啟動時只重新檢查授權。
- 替代方案：把 Guard 也移到 `BeginLaunch` 重新檢查（超出本票範圍，且票 38 已驗證通過）。
- 影響：「准入」在 Guard 與授權兩處指的交易不同，日後若要統一，需另開票調整 AI-208 或 AI-238。
- 審閱：未審

### AI-245：票 24 整合 rebuild/v1（票 20、35）：AI 編號順延為 AI-238～AI-244、migration 第 23 個、權限請求記下的授權版本改為 Run 准入時的 revision
- 重要度：中
- 時間／階段：2026-10-22，票 24 整合（run R085-24-integrate）
- 決策：(1) rebuild/v1 在本票驗證（R081）之後併入票 20（AI-215～AI-226，migration 21 `task_workdir_and_integration_queue`）與票 35（AI-227～AI-237，migration 22 `decision_center_full`）。本票的 AI-215～AI-221 順延為 AI-238～AI-244，移到 AI-237 之後；AI-243（原 AI-220）與 AI-244（原 AI-221）中指向本票規則的引用改為新編號（AI-219 → AI-242、AI-215 → AI-238），記錄歷次整合的歷史編號保留原文。自動合併再次吃掉 AI-244（原 AI-221）的 `- 審閱：未審` 行，重建時補回。本票的 migration `run_admission` 順延為第 23 個，開發指南同步改為 23。(2) `RunContext` 同時帶票 20 的 `workdirs` 與本票的 `settings_version`；`DecisionContext` 同時帶票 35 的 `authority` 與本票的 `authorizations`；工作面板同時回傳票 20 的 `workdirs`、`integrations` 與本票的 `authorization_revision`，Task 卡片同時顯示工作目錄與「沿用舊授權」。(3) 票 35 為權限提示的裁定請求記下 `target.authorization_revision`（「判定當時的授權」，裁定中心顯示「授權第 N 版」），原本取 Workspace 目前生效的 revision；整合後改為 Run 准入時固定的 revision，也就是權限提示實際依據的版本（AI-240）。票 35 的 migration 22 為既有權限請求補上的值（Workspace 最大 revision）不變。
- 理由：整合分支既有的條目與 migration 編號不可改動，順延尚未合併的本票。R1-a 下沿用舊授權的 Run 是依它准入時的 revision 判定範圍外，若請求記下目前的 revision，裁定中心會顯示一個判定時沒有用到的版本。
- 替代方案：保留票 35 的取法（撤權或擴權之後，舊 Run 提出的請求顯示的授權版本與判定依據不一致）；連同 migration 22 的回填一起改（要修改整合分支已有的 migration）。
- 影響：沒有撤權或擴權時兩者相同，只有沿用舊授權的 Run 提出的權限請求顯示的授權版本不同。
- 審閱：未審

### AI-246：worktree 自動清理的觸發條件：Task 已完成、最新整合項目已整合、這個 Task 的 Run 全部結束
- 重要度：高
- 時間／階段：2026-10-22，票 21 實作
- 決策：DR-20261021-02 的條件依序對應為：「通過驗證、成果已整合」＝ Task 是 `completed` 且它最新的整合項目是 `integrated`（開發 Task 只有整合後才會完成，見 AI-219）；「相關程序停止、無其他使用者」＝這個 Task 的 Run 全部是結束狀態（一個 Task 一個 worktree，見 AI-218，沒有其他 Task 使用同一目錄）；「必要成果與證據已保存於 worktree 外」由現有設計保證：Artifact 登記時已複製到成果儲存、驗證與整合證據在控制資料庫、整合結果在目標分支上，worktree 中唯一可能遺失的是未提交的檔案，由每次嘗試前的安全核對（AI-247）擋下。只清理平台登記且可用的 worktree；研究與文件 Task 的輸出目錄不清理。條件成立時由 Supervisor 在整合之後的同一個背景工作中登記 `pending`（事件 `worktree_cleanup_requested`）。
- 理由：都是已持久化的事實，Domain 可以純函式判定，崩潰後重算結果相同；暫停、交接、失敗、取消與完全退出都不會讓 Task 完成，自然不觸發清理。
- 替代方案：在整合完成的同一個交易中登記待清理（耦合整合佇列；Run 未全部結束時還要另外等待）；另外檢查完全退出的停止異常紀錄（完成的 Task 不會有進行中的 Run，停止異常只發生在退出當時未結束的 Run）。
- 影響：Task 若有任何 Run 停在非結束狀態（例如未確認停止），worktree 不會被清理，也不會顯示待清理。
- 審閱：未審

### AI-247：每次嘗試前的安全核對：必須仍是 repo 登記、簽出平台分支的 worktree，且沒有未提交的變更或未追蹤的檔案；被忽略的檔案不算
- 重要度：高
- 時間／階段：2026-10-22，票 21 實作
- 決策：每次嘗試先持久化 `removing`，再經 `WorkspaceFs::inspect_worktree` 核對：目錄不存在 → 安全（只剩平台自己的殘留要收拾）；是 repo 的 `git worktree list` 中非主 checkout 的項目、簽出 `baton/<Task>`，而且 `git status --porcelain --untracked-files=all` 是空的 → 安全；其他（主 checkout、普通目錄、簽出別的分支或 detached HEAD、有未提交的變更或未追蹤的檔案）→ 不移除，原因（最多列 5 個檔案）記在 `failure`。被 `.gitignore` 忽略的檔案（建置輸出、`node_modules` 等）視為可重建，一併刪除。核對前先 `git worktree prune`，避免舊登記剛好對到後來出現在同一路徑的其他目錄。
- 理由：「未保存變更或歸屬不明檔案阻擋刪除」；忽略的檔案若也阻擋，幾乎每個建置過的 worktree 都永遠不會被清理。
- 替代方案：被忽略的檔案也阻擋（磁碟空間無法回收）；只檢查追蹤檔的修改（未追蹤的新檔案會被刪掉）。
- 影響：使用者若把重要檔案放在被忽略的路徑中，清理時會一起刪除。
- 審閱：未審

### AI-248：移除方式：改名到 `.baton\trash\<Task>`、`git worktree prune`、再刪除暫放目錄；不用 `git worktree remove --force`，不刪除分支
- 重要度：高
- 時間／階段：2026-10-22，票 21 實作
- 決策：`WorkspaceFs::remove_worktree(repo, target, trash)` 先確認 `target` 是 repo 登記的非主 checkout worktree（否則拒絕、不刪任何檔案），把整個目錄改名到 `<Workspace>\.baton\trash\<Task>`，執行 `git worktree prune` 清掉 Git 的 worktree 登記，最後解除唯讀並刪除暫放目錄。冪等：`target` 已不存在時只做 prune 與刪除暫放目錄。平台分支、Git 歷史與成果都不動。Port 的 `inspect_dirty`／`DirtyState` 改為 `inspect_worktree`／`WorktreeState`，`remove_worktree` 多了 `repo` 與 `trash` 參數（兩者的正式版原本都是尚未實作）。
- 理由：Windows 上目錄內有檔案被占用時改名會整個失敗、什麼都不變，留下的狀態最容易說明；改名之後才刪除，崩潰或刪到一半失敗時，剩下的只會是平台自己的暫放目錄，重建後重新核對時原路徑已是「不存在」，不會把刪到一半的 worktree 誤判成「有未保存的變更」而卡住，也不需要 `--force`。
- 替代方案：`git worktree remove`（刪到一半失敗時工作目錄殘缺，之後的核對會看到大量「已刪除」的變更）；`git worktree remove --force`（會連未保存的檔案一起刪）。
- 影響：暫放目錄與 worktree 在同一個磁碟區（都在 `.baton` 內），改名不會跨磁碟複製。
- 審閱：未審

### AI-249：清理失敗的重試：沿用 D-16 的基礎設施重試預算（5 次，1、2、4、8、10 分鐘），用完即「清理失敗」，沒有手動重試
- 重要度：中
- 時間／階段：2026-10-22，票 21 實作
- 決策：安全核對不通過或移除失敗都回到 `pending`，記下原因與 `next_attempt_at`（`worktree_cleanup_retry_scheduled`）；第一次嘗試加上 5 次重試都失敗時進入 `failed`（`worktree_cleanup_failed`），不再自動重試，worktree 保留。崩潰後接續停在 `removing` 的那一次嘗試不另外計算（D-16：恢復嘗試不計入預算）。清理狀態與 Task 分開保存，任何結果都不改變 Task 的 `completed`。第一版不提供「重試清理」的操作。
- 理由：票面要求有限重試並顯示原因；D-16 已有基礎設施重試的數字。手動重試需要新增 API、桌面殼指令與 UI，屬於後續的空間管理面板（spec 已列為延後）。
- 替代方案：未保存變更的阻擋不計入次數、無限期等待使用者處理；提供手動重試按鈕。
- 影響：關掉占用檔案的程式超過約 25 分鐘後，worktree 會停在「清理失敗」，需要使用者自行刪除（之後有空間管理面板時再補手動重試）。
- 審閱：未審

### AI-250：完全退出中或等待「繼續工作」的 Workspace 不登記也不執行清理
- 重要度：中
- 時間／階段：2026-10-22，票 21 實作
- 決策：登記待清理與處理到期清理都跳過 `lifecycle::halt_for` 有阻擋原因的 Workspace（與 AI-217 準備工作目錄相同）；使用者「繼續工作」之後接著處理。被完全退出停下的 Task 回到 `ready`，本來就不會觸發清理。只登記使用中 Workspace 的 worktree；已登記的清理在 Workspace 封存後仍會處理。
- 理由：票面要求完全退出不觸發清理；停止期間不應執行 git 或刪除檔案。
- 替代方案：已登記的清理在停止期間照常執行（停止意圖期間仍有外部副作用）。
- 影響：Workspace 一直等待「繼續工作」時，已完成 Task 的 worktree 會一直保留。
- 審閱：未審

### AI-251：清理登記的資料形狀與顯示：獨立資料表 `worktree_cleanup`、工作面板 `workdirs[].cleanup`、四種狀態
- 重要度：低
- 時間／階段：2026-10-22，票 21 實作
- 決策：新增 migration `worktree_cleanup`（與 `task_workdir` 一對一，自己的聚合版本與事件 `worktree_cleanup_requested`／`_started`／`_retry_scheduled`／`worktree_cleaned`／`worktree_cleanup_failed`），不在 `task_workdir` 加欄位。狀態是 `pending`（待清理）、`removing`（清理中）、`cleaned`（已清理）、`failed`（清理失敗）；User Story 37 的三種顯示之外多了「清理中」，是為了崩潰後能辨識「這一次嘗試已開始」。工作面板 `GET tasks` 的 `workdirs[]` 多一個 `cleanup`（沒有登記時為 `null`），Web UI 在工作目錄區塊顯示清理狀態、上次失敗原因與下次自動重試時間；已清理時路徑標為已移除。`task_workdir.status` 不變（已完成的 Task 不會再派工）。
- 理由：工作目錄登記由準備流程整列覆寫（upsert），把清理狀態放在另一張表，兩邊的版本與事件互不干擾。
- 替代方案：在 `task_workdir` 加清理欄位並新增 `removed` 狀態（要重建 CHECK 約束）。
- 影響：備份整份複製控制資料庫，新表自然包含在內；還原後停在 `removing` 的清理會重新核對後接續。
- 審閱：未審

### AI-252：票 21 整合 rebuild/v1（票 35、24）：migration 順延為第 24 個、AI 編號順延為 AI-246～AI-252
- 重要度：中
- 時間／階段：2026-10-22，票 21 實作
- 決策：實作期間整合分支先後併入票 35（第 22 個 migration `decision_center_full`，AI-227～AI-237）與票 24（第 23 個 migration `run_admission`，AI-238～AI-245）。本票的 `worktree_cleanup` 順延為第 24 個，排在最後；三者沒有共用的資料表。本票條目原本編為 AI-238～AI-244，順延為 AI-246～AI-252，移到 AI-245 之後，條目內引用本票編號之處一併更新；整合分支的條目逐字未動。衝突合併方式：Store 的匯出同時保留本票的 `CleanupApplyError`、`CleanupWork` 與票 35 的 `DecisionOverview`；開發指南測試清單的情境測試兩邊都保留。
- 理由：已進入整合分支的 migration 與條目編號不可改動。
- 替代方案：無（編號規則已定）。
- 影響：本票較早的 commit 中 migration 註解寫「第 22 個」「第 23 個」、AI 編號寫 AI-238～AI-244，是改號前的編號；以本票分支（合併前）建立過的開發資料庫需刪除重建。
- 審閱：未審

### AI-253：外部操作由 Worker 執行，送出前後都向 Runtime 登記；Runtime 不代為執行、不自動查證
- 重要度：高
- 時間／階段：2026-10-22，票 26 實作
- 決策：新增兩個 Agent 工具：`begin_external_operation`（送出前以冪等鍵、操作種類、目標與參數登記，只有回 `proceed` 才可以送出）與 `record_external_operation_result`（送出後回報 `succeeded`＋回執、`failed`＋原因或 `unknown`＋原因）。外部操作仍由 Worker 用自己的工具執行（受 Workspace 授權與權限提示約束，與票 22、23 相同）；Runtime 只記錄 `external_operation`，判斷能不能再送、何時變成結果未知，不呼叫任何外部系統，也不自動查證。工具說明要求「無法確定是否生效一律回 unknown，不要自行重送」。
- 理由：spec 的 Port 清單沒有外部操作的 Port，Runtime 也看不到 Worker 工具的執行結果（AI-182）；要讓「冪等鍵、要求雜湊、回執、結果未知」成立，唯一能取得這些事實的是 Worker 本身。登記放在送出之前，Runtime 才能在崩潰後知道「可能已經送出」。
- 替代方案：Runtime 以 Outbox 加新的 Port 代為執行外部操作（每種外部系統都要 Adapter，遠超本票範圍）；只靠權限提示攔截（看得到要求、看不到結果，無法區分成功與未知）。
- 影響：保證只涵蓋有登記的操作；Worker 不登記就直接送出的操作，Runtime 只能透過權限提示與授權約束。Context Builder（票 39）組 Prompt 時應提醒 Worker 使用這兩個工具。
- 審閱：未審

### AI-254：結果未知的時機與處置：未回報的操作在 Run 結束的同一個交易轉為未知，建立裁定、相關 Task 進入 waiting_decision
- 重要度：高
- 時間／階段：2026-10-22，票 26 實作
- 決策：(1) 兩種情形變成 `unknown`：Worker 回報 `unknown`；Run 結束（結束回報、lease 核對後中斷、取消、完全退出、還原，全部經過 `runs::save`）時還有尚未回報結果的操作——在**同一個交易**中轉換，不另開步驟。(2) 每個未知操作建立一個裁定請求（識別 `decision_<操作>_a<嘗試>`，緊急程度 high），選項只有 `verified_done`「已查證：操作已經生效（不要重送）」與 `verified_not_done`「已查證：操作沒有生效（允許之後再執行一次）」，**不給建議**。資料庫的 `decision_request.kind` 以 CHECK 限定 `agent`／`permission`，所以種類記為 `agent`，與外部操作的關聯記在 `external_operation.decision_request_id`，沒有重建裁定表。(3) 相關 Task 若在 `running`、`ready` 或 `verifying` 就進入 `waiting_decision`；為此 Task 狀態機新增 `ready → waiting_decision`（Run 中斷時 Task 先被退回 ready，同一個交易再停下）。`blocked`、已在等裁定或已結束的 Task 維持原狀。(4) 回答時查證結果與 Task 恢復在同一個交易：`verified_done` → 操作 `succeeded`、之後同一個鍵回 `already_succeeded`；`verified_not_done` → 操作 `failed`、之後可以再嘗試一次。Task 依票 22 的規則回到 `ready` 並重新排隊。
- 理由：驗收條件「結果未知時相關 Task 阻擋，獨立 Task 繼續」；D-15 把「結果未知外部操作的處置」列為必須送裁定。若在 Run 結束後另開步驟才標記未知，中間 Task 會短暫停在 ready，可能被 Scheduler 重新派出。不給建議是因為 Runtime 對結果一無所知，任何建議都可能讓使用者不查證就按下。
- 替代方案：新增裁定種類 `external_operation`（要重建有觸發器與外鍵引用的 `decision_request` 表）；未知時讓 Task 進入 `blocked`（`blocked` 沒有「回答後恢復」的路徑，需要另做解除機制）；Run 結束後由 Supervisor 另一步驟掃描（有空檔）。
- 影響：等待驗證中的 Task（Run 已成功）若有未知操作，回答後回到 `ready` 重新派工，而不是直接回到 `verifying`。`blocked` 的 Task 若被使用者改回 ready 再派出，新 Run 登記同一個鍵仍會得到 `result_unknown`，不會重送。
- 審閱：未審

### AI-255：冪等鍵與要求雜湊的規則：範圍是 Workspace，鍵相同內容不同一律拒絕，跨 Task 共用結果
- 重要度：中
- 時間／階段：2026-10-22，票 26 實作
- 決策：冪等鍵在 Workspace 內唯一（`UNIQUE (workspace_id, idempotency_key)`）。要求雜湊是「操作種類、目標、參數（JSON，物件鍵排序）」的 SHA-256，不含冪等鍵。同一個鍵、雜湊不同回 409 `idempotency_key_conflict`，不覆寫。同一個鍵的登記結果：`succeeded` → `already_succeeded`（附回執，跨 Task 也一樣）；`failed` → 新的嘗試（`attempt` 加一，紀錄改屬目前的 Task 與 Run）；`in_progress` 且是同一個 Run → `proceed` 加 `existing: true`（登記的回應遺失時可安全重送）；`in_progress` 屬於另一個仍在執行的 Run → `in_progress_elsewhere`；`unknown` → `result_unknown`，且呼叫者的 Task 加入同一個裁定請求的阻擋範圍、進入 `waiting_decision`。回報結果只接受登記該操作的同一個 Run；重送同樣的結果冪等，改成別的結果 409 `external_operation_already_recorded`。`failed` 只代表「確定沒有生效」。欄位上限：鍵 200、操作種類 100、目標 500 字元、參數 16 KiB、回執 2000、原因 1000 字元。
- 理由：與 `message_id`（AI-129）一致；Worker 換 Run、換帳號或 Runtime 重啟後重試同一件事時需要穩定的鍵。鍵相同內容不同多半是 Worker 誤用，覆寫會讓原本的回執失去意義。
- 替代方案：鍵的範圍是 Task（不同 Task 重複部署同一版本時擋不住）；鍵相同內容不同時視為新操作（破壞冪等）。
- 影響：不同的事必須用不同的鍵；工具說明已提示。
- 審閱：未審

### AI-256：Outbox 啟動路徑：寫入 launch intent 時重查准入；spawn 失敗先查證；無法查證時留在 starting，不中斷也不重派
- 重要度：中
- 時間／階段：2026-10-22，票 26 實作
- 決策：(1) `BeginLaunch` 的交易除了票 25 的停止意圖，再以目前生效的授權、契約需要的能力與 Workspace 是否封存做一次准入（`authorization::admit`）；不成立就不寫入 launch intent，Run 以 `failed`（`launch_admission_denied：…`）結束、Outbox 視為處理完畢、Task 回 `ready`。為此 Run 狀態機新增 `queued → failed`。Supervisor 看到回傳的 Run 不是 `starting` 就不簽發憑證、不啟動。准入通過時 `run_launch_started` 事件記下依據的授權 revision。(2) `spawn` 回報失敗時先以 `command_id` 呼叫 `find`：找到（回執遺失）就補寫回執；`Ok(None)` 才判定 `launch_failed`；`find` 也失敗就留在 `starting`。(3) Runtime 重建後 Run 停在 `starting` 而 `find` 失敗時，維持既有行為：留在 `starting`、繼續持有寫入所有權，每次處理再查，沒有逾時上限。
- 理由：GLOSSARY「准入」定義為啟動前最後一次檢查，建立 queued Run 不等於已准入；票面要求「每條路徑都先檢查停止意圖與有效准入」「無法證明程序是否已啟動時進入 reconciliation 而非重派」。spawn 的錯誤不一定代表沒有程序（Port 規格寫明回傳前就已標記 command_id）。
- 替代方案：准入不成立時讓 Run 留在 queued 等授權恢復（每次處理都會重試且佔著寫入所有權）；改成 `cancelled`（語意是使用者取消）；`find` 持續失敗一段時間後中斷 Run（等於猜測沒有啟動，可能造成第二個 writer）。
- 影響：ProcessControl 長期無法查證時，該 Task 會一直停在 `running`，需要使用者處理（目前只能取消 Run，取消同樣需要確認程序停止）。
- 審閱：未審

### AI-257：寫入故障：依 SQLite 錯誤代碼判定，試寫成功才解除；故障期間不啟動程序也不排程
- 重要度：中
- 時間／階段：2026-10-22，票 26 實作
- 決策：(1) Store 的寫入佇列在每個寫入失敗時依錯誤代碼歸類：`SQLITE_FULL` → `disk_full`、`SQLITE_BUSY`／`SQLITE_LOCKED` → `busy`、`SQLITE_IOERR`／`SQLITE_READONLY`／`SQLITE_CANTOPEN` → `unwritable`；其他錯誤不算。故障狀態只存在記憶體（資料庫寫不進去），Runtime 重啟後由第一次失敗的寫入重新偵測。(2) 只有試寫單列表 `storage_probe` 成功才解除；沒有寫入任何東西的交易不算恢復。Supervisor 每次處理先試寫，故障期間不啟動任何程序（Outbox 第 1 步）也不排程，其他步驟（收事件、取消、核對 lease）照常。(3) 會寫入的 API 回 503，錯誤代碼是 `storage_disk_full`／`storage_busy`／`storage_unwritable`，訊息附處理方式；`GET /api/v1/health` 新增 `storage`（`dispatch_halted` 與故障的種類、摘要、處理方式、SQLite 原始訊息），Web UI 狀態列顯示。(4) 情境測試以 Store 的故障注入（`inject_write_fault`，產生與 SQLite 相同代碼的錯誤、走同一條歸類路徑）模擬；真實的 `SQLITE_FULL`（以 `max_page_count` 限制大小）與 `SQLITE_BUSY`（另一個連線持有 EXCLUSIVE 鎖）另有 Store 單元測試。
- 理由：使用者故事 85「磁碟滿或資料庫不可寫時新派工停止並顯示可處理原因，不會產生無法追溯的副作用」；不啟動程序就不會有回執寫不回去的程序。harness 只替換 Port（D-04），Store 是真實的，所以在 Store 加測試用注入點，而不是另做 Port。
- 替代方案：以真實的磁碟滿重現（不可控、會影響開發機）；任何寫入成功就解除（唯讀交易也會「成功」，造成誤判恢復與狀態閃爍）；故障時停止整個 Supervisor（取消與停止程序也會跟著停）。
- 影響：busy timeout 是 5 秒（AI-30），被鎖住時 API 要等 5 秒才回 503。磁碟滿時日誌每秒記一次仍無法寫入。
- 審閱：未審

### AI-258：外部操作的 API、資料表與事件形狀（migration 25 `external_operation`）
- 重要度：低
- 時間／階段：2026-10-22，票 26 實作
- 決策：Agent 工具 `POST /agent/v1/tools/begin_external_operation`（回 200 `{operation_id, status: proceed|already_succeeded|result_unknown|in_progress_elsewhere, attempt?, existing, receipt?, decision_request_id?, run_id?, message}`）與 `…/record_external_operation_result`（回 200 `{operation_id, status, decision_request_id?, message}`），MCP shim 一併提供（共 13 個工具）。桌面端點 `GET /api/v1/workspaces/{id}/external-operations` 回 `{operations}`。錯誤代碼：400 `external_operation_invalid`、404 `external_operation_not_found`、409 `idempotency_key_conflict`、`external_operation_not_in_progress`、`external_operation_already_recorded`、`run_report_stale`。資料表 `external_operation`（不可刪除）與 `storage_probe`。事件聚合 `external_operation`：`external_operation_begun`、`_succeeded`、`_failed`、`_result_unknown`、`_verified`。Web UI 本票只加狀態列的寫入故障提示；結果未知的操作透過既有的裁定中心呈現，沒有另做外部操作頁面。
- 理由：沿用 Agent API 與裁定中心的既有形狀；登記的四種結果都是正常回應（不是錯誤），Worker 依 `status` 分流，與 `permission_prompt` 的等待裁定回 200 一致。
- 替代方案：`result_unknown` 回 409（MCP shim 會把它包成工具錯誤，Worker 容易當成暫時失敗而重試）。
- 影響：無。
- 審閱：未審

### AI-259：票 26 整合 rebuild/v1（票 14、38、20）：migration 順延為第 22 版、AI 編號順延為 AI-253～AI-259
- 重要度：低
- 時間／階段：2026-10-22，票 26 實作 run R072-26-impl
- 決策：(1) 第一次整合：整合分支已有第 19 個 migration（票 14 `model_overrides_and_native_session`）與第 20 個（票 38 `guard`）。`supervisor.rs` 的衝突保留整合分支新增的啟動步驟（設定快照、`prepare`、步驟編號改為 5），再套上本票「spawn 失敗先查證」的邏輯；`Run` 多了 `native_session_id`，本票的 Domain 測試輔助補上 `None`。(2) 第二次整合：整合分支又併入票 20（第 21 個 migration `task_workdir_and_integration_queue`，AI-215～AI-226），本票的 `external_operation` 順延為第 22 個、排在最後；本票條目由 AI-215～AI-221 順延為 AI-253～AI-259，移到 AI-226 之後，條目內引用本票編號之處與 migration 序號一併更新。開發指南的目錄表與 harness 測試清單兩邊的新增內容都保留。程式碼沒有引用這些編號。本次只整合、不新增行為。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票。
- 替代方案：無（編號規則已定）。
- 影響：用本票分支整合前建過的開發資料庫（`external_operation` 記在第 19 或 21 版）需要刪除重建。
- 審閱：未審

### AI-260：票 26 整合 rebuild/v1（票 35、24、21）：migration 順延為第 25 個、AI 編號順延為 AI-253～AI-259
- 重要度：低
- 時間／階段：2026-10-22，票 26 整合 run R088-26-integrate
- 決策：(1) 整合分支已有第 22 個 migration（票 35 `decision_center_full`，重建裁定四張表）、第 23 個（票 24 `run_admission`）與第 24 個（票 21 `worktree_cleanup`）。本票的 `external_operation` 順延為第 25 個、排在最後；它以外鍵引用 `decision_request (id)`，在票 35 重建該表之後才建立，不受重建影響；四者沒有共用的資料表，都會套用。(2) 本票條目 AI-227～AI-233 與整合分支票 35 的 AI-227～AI-237 重號，順延為 AI-253～AI-259，移到 AI-252 之後；條目內引用本票編號之處與 migration 序號一併改為現行編號（與 AI-226、AI-259 的處理相同），當時的整合經過照原文保留。整合分支的條目逐字未動；程式碼與文件沒有引用本票的編號。(3) 衝突合併：`DecisionRequest` 改用票 35 的形狀（目標版本、內容版本、延後、合併、連結），本票建立「結果未知」請求的 `requested_event` 改為走請求聚合的事件版本，`insert_request` 改寫票 35 的 24 欄；`Store::answer_decision` 改帶 `expected_revision` 與 `Resolution`，本票 store 與 harness 測試的作答輔助函式隨之改為先讀內容版本、再以「接受」作答（斷言不變）；`resolve_by_decision` 移到單筆與批次共用的 `apply_in`，批次作答同樣會結算外部操作；MCP shim 現在有 14 個工具（票 35 的 `controller_action` 加上本票的兩個），兩邊的工具清單測試合併並改為 14；開發指南的目錄表、Run 狀態與情境測試清單以整合分支為底補上本票內容。行為上的取捨另記於 AI-261、AI-262。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票。
- 替代方案：無（編號規則已定）。
- 影響：用本票分支建過的開發資料庫（`external_operation` 記在第 19、21、22 或 24 版）需要刪除重建。本票較早的 commit 訊息與 `.orchestration` 下的驗證紀錄（R087）提到的 AI-227～AI-233 是改號前的編號。
- 審閱：未審

### AI-261：啟動准入被拒的結束方式統一採用票 24：`interrupted`（`admission_denied`），取消本票的 `queued → failed`
- 重要度：中
- 時間／階段：2026-10-22，票 26 整合 run R088-26-integrate
- 決策：本票（AI-256）與票 24（AI-238、AI-239）各自在 `BeginLaunch` 的交易中重查准入。整合時只保留票 24 的實作：准入被拒時 Run 以 `interrupted` 結束，`end_reason` 以 `admission_denied` 開頭，事件是 `run_admission_denied`，Task 回 `ready` 並在同一個交易重新排隊；派工 Outbox 同樣視為處理完畢，不寫 launch intent、不簽發憑證、不啟動程序。本票新增的 `queued → failed` 轉換、`LAUNCH_ADMISSION_DENIED` 常數與重複的 `admit_at_launch` 移除。本票的 Domain 測試與情境測試 `a_run_whose_authorization_is_revoked_before_launch_ends_without_starting_a_process` 改為斷言 `interrupted`／`admission_denied`；「沒有 launch intent、沒有 spawn、沒有憑證、Outbox 處理完畢」的斷言不變。開發指南〈Supervisor〉第 1 步同步改寫。
- 理由：兩張票規範的是同一個時點（准入 = launch intent 持久化），只能有一種結果；票 24 已在整合分支，而且它的「自動重新排隊、以授權類別顯示等待原因」比本票的 `failed` 完整。准入被拒不是執行失敗，記成 `failed` 會被 Guard 的重試政策（票 38）當成失敗的 Run 計算。
- 替代方案：保留本票的 `failed`（要改票 24 已合併的行為與測試）；兩種並存（同一個時點有兩種結果，無法判斷）。
- 影響：AI-256 第 (1) 點關於 `queued → failed` 與 `launch_admission_denied` 的描述由本條取代，其餘照舊。
- 審閱：未審

### AI-262：外部操作的查證請求只能以「接受查證選項」或「延後」處理；不能合併、不會過期
- 重要度：高
- 時間／階段：2026-10-22，票 26 整合 run R088-26-integrate（處理 R087 驗證報告第 1 點）
- 決策：票 35 讓裁定請求有六種處理方式，也可以合併與過期。「結果未知」的查證請求（AI-254）若被駁回、回答、要求調查或修改條件，請求會結案、Task 恢復，但外部操作仍停在 `unknown`、指向已結案的請求：同一個冪等鍵之後永遠回 `result_unknown`，也沒有人能再查證。因此：(1) `DecisionRequest` 新增 `external_operation_id`（不存在裁定表，由 Store 載入時依 `external_operation.decision_request_id` 對回，沒有改 schema）；有值時 `DecisionRequest::allowed_actions` 只回 `accept`、`defer`。Domain 的作答檢查與裁定中心顯示的 `allowed_actions` 都改用它，所以單筆、批次與 UI 一致，其他方式回 400 `decision_action_not_allowed`。選項本來就只有 `verified_done`／`verified_not_done`，「接受」一定落在其中一個。`modify` 也不允許：查證沒有可修改的條件，備註可以隨「接受」附上。(2) 合併：查證請求不能合併到別的請求，別的請求也不能合併進來（400 `decision_merge_invalid`），因為每個請求對應一個外部操作；需要對照時可以用連結。(3) 過期：查證請求不因目標 Task 的契約改版或 Task 結束而標為 `superseded`——外部操作有沒有生效與 Task 契約無關，過期會造成和 (1) 相同的卡死。(4) 新增 Domain 測試 `verification_requests_can_only_be_answered_by_accepting_an_option_or_deferred`、`verification_requests_never_go_stale_and_cannot_be_merged`，以及情境測試 `an_unknown_operation_can_only_be_closed_by_verifying_whether_it_took_effect`（單筆的四種方式與批次的駁回都被拒；延後之後請求仍待處理、操作仍 `unknown`、Task 仍等待；查證後才結案）。
- 理由：驗收條件「外部操作結果未知時相關 Task 阻擋」與 D-15「結果未知外部操作的處置必須送裁定」；只有查證結果能讓操作離開 `unknown`。
- 替代方案：新增裁定種類 `external_operation`（要再重建票 35 剛重建的 `decision_request` 表，CHECK 與觸發器都要改）；允許其他方式，並在結案時讓操作回到「可再查證」（要另做重新開啟請求的機制）；以請求識別的前綴判斷（脆弱）。
- 影響：Web 裁定中心依 `allowed_actions` 只顯示「接受」與「延後」。合併選單只看種類，查證請求若被手動連結到同種類的請求，選單仍會列出合併目標，送出時由 Runtime 拒絕並顯示錯誤。權限提示的處理方式限制（AI-231）照舊。
- 審閱：未審

### AI-263：沿用或替換 Session 在寫入 launch intent 的交易中決定；接續能力未知一律替換
- 重要度：高
- 時間／階段：2026-10-22，票 15 實作
- 決策：`BeginLaunch` 新增 `working_dir` 與 `resume_check`，Domain 在同一個交易中決定沿用或建立 Session（`session::plan_launch`）。Supervisor 寫入前先找「候選」：同一個 Task 最近的 Session，且帳號、工作目錄相同、已有原生識別、不是 closed／unavailable；只對候選呼叫 `AgentAdapter::resume_capability`（`SessionRef` 新增 `config_dir`、`working_dir`）。只有查驗結果為 `Resumable` 才沿用（新 Run、同一個 Session、`run.resume_native_session_id` 固定為原生識別、啟動參數帶 `--resume`）；`NotResumable` 前任標 `unavailable`、`Unknown` 或查驗失敗前任標 `closed`，都建立替代 Session。讀不到候選（資料庫錯誤）時這次不啟動，下一次 tick 再試。新 Session 的識別由 Run 導出（`ses_<run 識別的亂數部分>`）。
- 理由：issue §11.3「啟動前保存平台 Session 與 command_id」與 §3.2「未知時不宣告可續用」；Session 與 launch intent 同交易，Runtime 在 spawn 前後當掉時只有一組持久化的決定，沿用 command_id 找回原程序的既有機制（票 10）。識別由 Run 導出讓重送同一個命令得到同一個識別。
- 替代方案：派工（Dispatch）時就決定（那時還不知道接續能力，啟動時還要再改一次）；查驗失敗時視為可接續（違反 §3.2）。
- 影響：真實 Adapter 依原生 Session 檔是否存在判斷（AI-268）；判斷不了時一律換新 Session，會損失原生對話但不會誤宣稱已恢復。
- 審閱：未審

### AI-264：前任 Session 的去向與紀錄缺口；替代 Session 的 Prompt 暫不附檢查點
- 重要度：中
- 時間／階段：2026-10-22，票 15 實作
- 決策：替換原因代碼：`account_changed`、`working_directory_changed`、`native_session_never_bound`、`native_session_unavailable`、`resume_capability_unknown`、`predecessor_not_resumable`（前任早已關閉且沒有記錄原因）。原生 Session 本身有問題（`native_session_never_bound`、`native_session_unavailable`）的前任標 `unavailable`，其餘（換帳號、換目錄、能力未知）標 `closed`；前任早已 closed／unavailable 時維持原狀，替代 Session 沿用前任記錄的原因。驅動 Run 結束（任何結果）時：有原生識別 → `suspended`，從未取得 → `unavailable`（`native_session_never_bound`）。每個有前任的 Session 都有 `history_gap` 說明「原生對話沒有延續、紀錄有缺口」。替代 Session 的 Prompt 仍是派工時固定的 Task 契約，不另外附上前任的檢查點或摘要。
- 理由：§3.3「舊紀錄標為 unavailable…保留失效原因，不覆蓋原 ID」；換帳號時原生 Session 本身可能仍完好，標 `closed`（不再相容）比 `unavailable` 精確。Prompt 在派工時固定進 Outbox，而替換在啟動時才決定；從檢查點重建的內容屬交接（票 28）與 Context Builder（票 39）。
- 替代方案：替換一律標 `unavailable`；在啟動時改寫 Prompt 附上最近的檢查點（需要改動派工 Outbox 的固定內容）。
- 影響：替代 Session 的 Worker 目前只拿到 Task 契約，不知道前任做到哪裡；票 28／39 需要補上。Task 完成或取消時不主動關閉 Session（Session 只隨 Run 改變），`last_checkpoint_id` 與相容性資料（執行工具版本、政策版本、資料來源版本）未保存，留給需要它們的票。
- 審閱：未審

### AI-265：同一原生 Session 只有一個執行者的三道防線；後到的 Run 被要求停止
- 重要度：高
- 時間／階段：2026-10-22，票 15 實作
- 決策：（1）平台 Session 以 `driver_run_id` 記錄目前驅動它的 Run（只在 allocating／active 時有值，資料表 CHECK 保證），有執行者時不能被沿用（`RunError::SessionBusy`，代碼 `session_busy`）；（2）資料庫唯一約束：同一個 Session 最多一個 `starting`／`running` 的 Run、同一個 `執行工具 + 帳號` 內一個原生識別只綁一個 Session、一個 Run 只驅動一個 Session；（3）Run 回報的原生識別已由同帳號另一個平台 Session 綁定時不綁定、記 `session_native_conflict`，若那個 Session 正由別的 Run 驅動，後到的 Run 在同一個交易中被要求取消（`run_cancel_requested`，`reason` = `native_session_in_use`），Supervisor 照取消流程停止程序。原生識別的命名空間是「執行工具 + 帳號」（每個帳號有自己的 `CLAUDE_CONFIG_DIR`，即 Session 儲存命名空間），綁定時另外載入其他 Workspace 中綁定了同一個識別的 Session。Run 上原本的綁定行為（AI-200）不變。
- 理由：票面驗收「同一原生 Session 不能被兩個有效執行者同時驅動」；寫入所有權（execution lease）已讓同一個 Task 只有一個 Run，但原生識別相同的情況可能跨 Task、跨 Workspace。停止後到者而不是先到者，是因為先到者已經在寫入。
- 替代方案：只記錄衝突不處置（兩個程序可能同時寫入同一個原生 Session）；讓後到的 Run 直接失敗（程序可能仍在執行，必須先確認停止）。
- 影響：被要求取消的 Run 依既有取消流程確認停止後才釋放寫入所有權，Task 回到 ready。
- 審閱：未審

### AI-266：session_events 以事件表的檢視實作；Session 新增的表與欄位
- 重要度：中
- 時間／階段：2026-10-22，票 15 實作
- 決策：migration 第 26 個（`agent_session_and_transcript`）：`agent_session`（issue §3.2 的欄位，另有 `driver_run_id`、`state_reason`、`history_gap`、`resume_capability`／`capability_checked_at`、`version`）、`transcript_segment`、`run.agent_session_id`（外鍵）與 `run.resume_native_session_id`。`session_events` 是 `event` 表中 `aggregate_type = 'agent_session'` 的檢視，Session 事件與其他聚合一樣追加、不可修改。`session_store_ref` 不另存（等於 Profile 的設定目錄，由 `profile_id` 取得），`working_directory_ref` 存實際路徑。端點：`GET /api/v1/workspaces/{id}/sessions` 與 `…/sessions/{session_id}`（`session_not_found`）；不做 UI。
- 理由：事件表已是追加式、帶版本的唯一事件來源，另建一張表會讓同一事件存兩份。UI 不在票面要求內。
- 替代方案：獨立的 `session_event` 表；在 Run 詳情畫面顯示 Session（後續要顯示時可直接用這兩個端點）。
- 影響：Session 只有 Worker（`role` = worker、`task_id` 有值）；Controller Session 與 `conversation_id` 由票 16 使用同一張表。
- 審閱：未審

### AI-267：對話紀錄取自 stream-json 輸出、以位置去重、遮蔽後以雜湊命名存成果儲存
- 重要度：高
- 時間／階段：2026-10-22，票 15 實作
- 決策：Port 新增 `AdapterEvent::Transcript { position, line }`；真實 Adapter 以 `stream.jsonl` 的每個非空白行為一筆，位置 = 第幾個非空白行（從 1 起，重新接上時重播得到相同位置）。Supervisor 每次 tick 把收到的紀錄交給 `session::plan_transcript_import`：位置 ≤ 已保存位置的略過、同批重複的去重、跳號記為 `missing` 段落；每一行遮蔽秘密（JSON 行逐一遮蔽解碼後的字串值，非 JSON 整行處理；規則沿用共同設定的秘密辨識，另把恰好 64 個十六進位字元的片段視為 Runtime 簽發的憑證），以內容的 SHA-256 命名經 `WorkspaceFs::write_artifact_atomic` 寫入 `<資料目錄>\artifacts\transcripts\<sha256>.jsonl`，再記錄 `transcript_segment`（Session、Run、位置區間、雜湊、大小、筆數、遮蔽數；同一個 Run 的區間不重疊）。寫入失敗時停在那一段，之後到達的位置會把沒保存的部分記為缺口。
- 理由：stream-json 是 Runtime 實際收到、可依位置重播的原生對話；原生 `projects\*.jsonl` 在 Profile 目錄內、格式未承諾，且讀它需要另一套追蹤。憑證值不在 Runtime 記憶體中（Runtime 重建後只剩雜湊），只能以形狀遮蔽。
- 替代方案：匯入原生 `projects\<cwd>\<id>.jsonl`；以 `credential` 表的雜湊比對每個 64 位十六進位字串（更精確，但匯入時要查詢資料庫）。
- 影響：SHA-256、Git 完整雜湊以外的 64 位十六進位字串也會被遮蔽。**備份目前不含 transcript 檔**（備份只複製 `artifact_version` 登記的成果），spec 驗收 15.2.7「備份還原後能取得保留的 transcript 產物」需要後續把 `transcript_segment` 的 stored 段落加入備份清單。Controller 的對話紀錄（票 16）沿用同一套。
- 審閱：未審

### AI-268：真實 Adapter 的 `--resume` 與接續能力判斷（已以 Work-3 實測）
- 重要度：中
- 時間／階段：2026-10-22，票 15 實作
- 決策：以 Work-3、`haiku`、極短 Prompt 實測 Claude Code 2.1.307：`claude -p --resume <id>` 的 init 與 result 都維持同一個 `session_id`，對話內容延續；原生 Session 檔在 `<CLAUDE_CONFIG_DIR>\projects\<工作目錄把每個非英數字元換成 ->\<id>.jsonl`；`--resume` 不存在的識別只輸出一行 `error_during_execution` 的 result（stderr：`No conversation found with session ID`）。Adapter：`RunSpec.resume_native_session_id` 有值時加 `--resume <id>`；`resume_capability`：識別只接受 ASCII 英數字、`-`、`_`（否則 `NotResumable`，不組路徑），`config_dir` 或 `working_dir` 缺少、`projects` 目錄不存在或讀不到、轉換後目錄名超過 200 字元 → `Unknown`，檔案存在 → `Resumable`，不存在 → `NotResumable`。實測後刪除了實測在 Work-3 留下的 `projects` 子目錄。
- 理由：只看 Session 檔是否存在是不啟動 CLI 就能做的最直接證據；很長的路徑 CLI 可能截斷並加雜湊（未實測），保守回 `Unknown` 會換新 Session，不會誤接。`C:x` 這類識別接進路徑會逃逸，所以用白名單。
- 替代方案：以 `claude -p --resume <id>` 試跑（耗用額度且有副作用）；一律回 `Unknown`（同帳號續跑永遠無法接續）。
- 影響：檔案存在但內容損毀時仍判為 `Resumable`，接續失敗會讓 Run 以 CLI 的錯誤結束，下一次派工再查驗。細節記在 `docs/adapter-contract.md`。
- 審閱：未審

### AI-269：同一個 Task 再次派工時優先留在上一個 Run 的帳號
- 重要度：中
- 時間／階段：2026-10-22，票 15 實作
- 決策：`scheduler::evaluate` 以同一個 Task 最高世代的 Run 的帳號作為偏好帳號，新增 `scheduler::place_preferring`：偏好帳號在可派的候選中（核對無誤、額度未達門檻、帳號名額未滿）就選它，否則照原規則（進行中 Run 最少、同數取較早建立）。`place` 不變（偏好為 `None`）。
- 理由：原生 Session 不能跨帳號接續；原規則在兩個帳號都空閒時一律選較早建立的帳號，會讓在第二個帳號上失敗的 Task 換帳號、失去原生 Session。
- 替代方案：不調整（同帳號續跑只在剛好選到同一帳號時發生）。
- 影響：重試的 Task 會集中在原帳號，帳號之間的負載平衡稍差；額度接近門檻或名額滿時仍會換帳號（並建立替代 Session）。
- 審閱：未審

### AI-270：票 15 整合 rebuild/v1（票 38、20、35、24）：migration 順延為第 24 個、AI 編號順延為 AI-253～AI-260、Session 的工作目錄以 Task 工作目錄為準
- 重要度：低
- 時間／階段：2026-10-22，票 15 實作
- 決策：整合兩次。第一次（票 38、20、35）：`RunContext` 與 Store 的 `Loaded` 同時帶 `workdirs`、`guards` 與 `sessions`；Session 的 `working_directory` 與沿用條件中的工作目錄，都是派工時固定進 Outbox 的 Task 工作目錄（票 20 起是 worktree 或輸出目錄），情境測試相應改為比對 Run 的實際工作目錄。第二次（票 24）：`BeginLaunch` 同時做啟動准入（票 24，被拒時不建立 Session，Run 直接結束）與 Session 決定（准入通過後才決定），Supervisor 先查驗候選 Session 再送出同一個命令；Run 欄位順序為票 24 的 `admitted_at`、`settings_version` 之後接本票的 `agent_session_id`、`resume_native_session_id`。本票的 migration 排在票 24 之後，為第 24 個；本票的 AI 編號由 AI-238～AI-245 順延為 AI-253～AI-260（較早的 commit 訊息與文件中的舊編號是改號前的）。
- 理由：同一個 Task 回到同一個 worktree，原生 Session 才接得回來（票 20 的設計），兩者一致；准入被拒的 Run 沒有程序，不需要 Session。
- 替代方案：無。
- 影響：worktree 被清理後（票 21）工作目錄改變或消失時，依「工作目錄不同」換新 Session；在已移除路徑直接 resume 的檢查由票 21 處理。用本票分支建過的開發資料庫，`agent_session_and_transcript` 記在第 23 版，與整合後的編號不一致，需要刪除重建。
- 審閱：未審

### AI-271：票 15 整合 rebuild/v1（票 21）：migration 再順延為第 25 個、AI 編號再順延為 AI-253～AI-260
- 重要度：低
- 時間／階段：2026-10-22，票 15 整合（run R091-15-integrate）
- 決策：本票驗證（R090）之後整合分支併入票 21（第 24 個 migration `worktree_cleanup`，AI-246～AI-252）。本票的 `agent_session_and_transcript` 順延為第 25 個，排在 `worktree_cleanup` 之後，兩者沒有共用的資料表；Store 註解與 AI-256（原 AI-249）的 migration 編號同步改為 25。本票條目 AI-246～AI-253 順延為 AI-253～AI-260，移到 AI-252 之後，條目內引用本票編號之處一併更新（例如 AI-253、AI-257 中的 AI-251 → AI-258）；AI-260（原 AI-253）中「migration 為第 24 個」是上一次整合當時的歷史，保留原文。整合分支的條目逐字未動。程式碼與文件中沒有引用本票 AI 編號之處。衝突合併方式：Supervisor 匯入的 Port 型別同時保留本票的 `Capability`、`SessionRef` 與票 21 的 `WorktreeState`。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票。
- 替代方案：無（編號規則已定）。
- 影響：本票較早的 commit 中 migration 註解寫「第 24 個」、AI 編號寫 AI-246～AI-253，是改號前的編號；用本票分支（本次合併前）建過的開發資料庫，`agent_session_and_transcript` 記在第 24 版，與整合後的編號不一致，需要刪除重建。
- 審閱：未審

### AI-272：票 15 整合 rebuild/v1（票 26）：migration 再順延為第 26 個、AI 編號再順延為 AI-263～AI-271
- 重要度：低
- 時間／階段：2026-10-22，票 15 整合（run R092-15-integrate）
- 決策：整合分支併入票 26（第 25 個 migration `external_operation`，含 `storage_probe`；AI-253～AI-262）。本票的 `agent_session_and_transcript` 順延為第 26 個、排在 `external_operation` 之後；兩者沒有共用的資料表（`external_operation` 只以外鍵引用 `run`，本票對 `run` 加的兩欄不影響它），都會套用。Store 註解與 AI-266（原 AI-256）的 migration 編號同步改為 26。本票條目 AI-253～AI-261 順延為 AI-263～AI-271，接在 AI-262 之後；實質條目（AI-263～AI-269）內引用本票編號之處改為現行編號（AI-263 中的 AI-258 → AI-268），兩則整合紀錄（AI-270、AI-271）只改標題編號，內文是當時的經過，保留原文。整合分支的條目逐字未動；程式碼與文件沒有引用本票的 AI 編號（`run/tests.rs` 與 `outbox_and_external_ops.rs` 中的 AI-261 是票 26 的條目，不變）。衝突合併：API 路由同時掛上本票的 `sessions` 與票 26 的 `external_operations`；開發指南目錄表以整合分支的 domain／store 敘述為底，補上本票的 `session.rs`、`sessions.rs`。自動合併的 Supervisor 與 `runs::save` 兩邊行為並存：啟動前先查驗候選 Session（本票）、spawn 失敗先以 command_id 查證與寫入故障時停止啟動和派工（票 26）；Run 結束的同一個交易同時結算 Session（本票）與未回報的外部操作（票 26）。票 26 新增的三個測試初始值補上本票新增的欄位：`BeginLaunch` 的 `working_dir`、`resume_check: None`（domain 測試改用既有的 `begin` 輔助函式），`Run` 的 `agent_session_id`、`resume_native_session_id` 為 `None`；斷言不變。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票。
- 替代方案：無（編號規則已定）。
- 影響：用本票分支（本次合併前）建過的開發資料庫，`agent_session_and_transcript` 記在第 25 版，與整合後的編號不一致，需要刪除重建。
- 審閱：未審

## Spec 決策審閱紀錄

### D-04：主要測試 seam
- 審閱：認同 2026-10-22

### D-05：Agent 與 Runtime 的工具介面
- 審閱：認同 2026-10-22

### D-02：本 spec 範圍與交付順序
- 審閱：認同 2026-10-22

### D-06：Controller 執行方式
- 審閱：認同 2026-10-22
