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

### AI-128：完全退出停下的 Run 以 interrupted 結束，Task 退回 ready 並自動重新排隊；停不下來的保留寫入所有權
- 重要度：高
- 時間／階段：2026-10-22，票 25 實作
- 決策：程序確認停止（`StoppedGracefully`／`ForceKilled`）或根本沒有程序（`queued`、啟動中但以 command_id 找不到）的 Run，以 `interrupted`（`end_reason = stopped_by_full_exit`）結束並釋放寫入所有權；Task 只在仍是 `running` 時退回 `ready`（`blocked`、`verifying` 等不動），並在**同一個交易**中重新放進派工佇列（優先序 0，原本的優先序已不存在）。使用者先前已要求取消的 Run 則完成取消（Task 不重新排隊）。`StillAlive`、停止呼叫失敗、到總上限仍沒有結果的記為停止異常，Run 維持 `running` 並保留 lease。重新開啟核對時，這些 Run 的程序若確定已不在，以 `process_gone_after_full_exit` 中斷、Task 回 `ready` 但**不**自動重新排隊；仍存活的維持原狀（不會再啟動第二個 Worker）。
- 理由：票面要求「完全退出不會把 Task 標為取消或完成」與「重開後 A 的合格工作自動恢復」，被退出停下的工作必須有恢復的入口；放回佇列後，「續跑」就只是解除暫停，由排程照常判定授權、相依、帳號與容量（即「合格」）。這和 AI-96「Run 失敗不自動重新排隊」不衝突：退出不是失敗，而是使用者明確要求的暫停。停不下來的程序若釋放 lease，續跑時會產生第二個寫入者（使用者故事 43）。查證後才知道已不在的程序，結果不明（它可能已完成部分工作），依 AI-67 的原則交給使用者重新派工。
- 替代方案：退出時把 Run 標為 `cancelled`（語意上是使用者取消，且 AI-66 會讓人誤以為 Task 被放棄）；不重新排隊，續跑時再掃描「被退出停下的 Task」（需要另外記錄哪些 Task 屬於這一類，且會和使用者的手動派工打架）；停止異常也釋放 lease（可能兩個寫入者）。
- 影響：被停下的 Task 在重新開啟前一直排隊、等待原因是 `runtime_stopping`；停止異常的 Task 在核對前保持 `running`。重新排隊不保留原本的優先序。
- 審閱：未審

### AI-129：停止意圖的保存方式與阻擋點：單列狀態加各 Workspace 暫停，擋在 Scheduler 與 launch intent
- 重要度：高
- 時間／階段：2026-10-22，票 25 實作
- 決策：停止意圖存在單列表 `runtime_lifecycle`（`phase` = `running`／`stopping`／`stopped`、`version`、最近一次停止報告 JSON），`stopping` 與 `stopped` 都算停止意圖生效，直到重新開啟。要求退出的同一個交易中，把每個使用中的 Workspace 標為暫停（`workspace_lifecycle.paused_since`）。阻擋點只有兩個：（1）Scheduler 的 `Capacity.halt`（`lifecycle::dispatch_halt`），`place` 一看到就回等待，所以佇列重試、額度恢復後的派工、API 的新派工要求與未來的交接（票 28 以 `SlotKind::Handoff` 呼叫同一個 `place`）都被擋；（2）寫入 launch intent（`BeginLaunch`）的交易再檢查一次（`RunError::LaunchHalted`），擋住停止意圖寫入前就已派出、還沒啟動的 Run。等待原因新增第七類 `lifecycle`（`runtime_stopping`、`awaiting_resume`）。上次退出後尚未重新開啟（`stopped`）時再按「完全退出」，視為新的退出要求（版本加一），停止中則冪等。
- 理由：ADR-0001 要求「排程、Outbox 與 supervisor 啟動路徑都必須檢查停止意圖，不能只停排程器」；把檢查放在所有派工都會經過的 `place` 與所有啟動都會經過的 `BeginLaunch`，新加入的路徑（交接、重試預算）不必各自記得檢查。暫停在退出時就寫入，重新開啟只需要決定「誰解除」，不必回頭推算哪些 Workspace 受影響。
- 替代方案：只在 Supervisor 背景迴圈檢查（API 派工、未來的交接會漏掉）；不記錄 Workspace 暫停、重新開啟時才逐一判定（重新開啟前新增的 Workspace 會被誤判，且「繼續工作」沒有可以清除的狀態）；把停止意圖放進 `RunContext`（要改動所有建構點，與排程等待原因的呈現脫節）。
- 影響：交接（票 28）與重試預算（票 36）只要經過 `scheduler::place`／`evaluate` 就自動遵守停止意圖；若另闢直接呼叫 `RunCommand::Dispatch` 的路徑，必須先呼叫 `lifecycle::dispatch_halt`。
- 審閱：未審

### AI-130：重新開啟的流程：先核對再解除；資料庫與生命週期檔取保守者；以版本防止清掉新的退出要求
- 重要度：中
- 時間／階段：2026-10-22，票 25 實作
- 決策：Runtime 啟動時讀資料庫與 `<資料目錄>\lifecycle.json`：只有檔案有停止意圖、或檔案存在卻讀不懂（毀損）都視為停止，並把停止意圖補記進資料庫（同時暫停每個使用中的 Workspace、記一筆說明）；只有資料庫有時把它鏡像回檔案。停止意圖仍生效時，Supervisor 的第一次 `tick` 先核對（處理啟動中的 Run：以 command_id 找回或中斷；對每個有程序身分的 Run 不論 lease 是否過期都 `inspect`，`Dead` 才中斷），再以「啟動時看到的生命週期版本」做條件式重新開啟：版本已改變（核對期間使用者又按了完全退出）就不重新開啟。重新開啟後把檔案改寫為沒有停止抑制。
- 理由：spec D-25「兩者不一致時採較保守者」；票面「重新開啟先核對，再依設定續跑」。核對放在 Supervisor 的同一把鎖裡，與一般處理不會交錯；版本比對避免「剛重新開啟的 Runtime 把使用者新的停止意圖清掉」。
- 替代方案：重新開啟在 `Runtime::start` 內同步完成（測試無法決定核對時機，且啟動會被慢的 `inspect` 卡住）；檔案毀損時忽略檔案（可能在使用者要求停止後被 launcher 自動啟動）。
- 影響：Runtime 啟動到第一次處理之間（正式執行約 1 秒），API 仍回報停止意圖生效、「繼續工作」回 409。重新開啟時寫檔失敗只記日誌：下次啟動時會再被視為停止（較保守），使用者需要再次重新開啟。
- 審閱：未審

### AI-131：「保存並停止」的通知方式：Agent 回報的回應帶 `stop_requested`，強制力仍靠程序樹終止
- 重要度：中
- 時間／階段：2026-10-22，票 25 實作
- 決策：停止意圖生效後，`report_progress`、`save_checkpoint`、`report_blocked` 的成功回應帶 `stop_requested: true`（平常是 `false`），Worker 據此保存 checkpoint 並結束。Runtime 不另外對程序送出任何訊號；停止流程對每個 Run 呼叫 `ProcessControl::stop(identity, 30 秒)`：寬限期內等程序自行結束，逾時以 Job Object 終止整棵程序樹（AI-72）。
- 理由：`claude -p` 執行中沒有可靠的輸入管道（stdin 在啟動時就導向 NUL，AI-72），送主控台中斷訊號需要與 Worker 共用主控台，且 Windows 上無法對特定程序樹可靠地送 Ctrl+C。回報的回應是 Worker 本來就會定期呼叫的管道，不需要新的工具或協定。
- 替代方案：新增 `should_stop` 工具讓 Worker 輪詢（Worker 要記得呼叫，提示也要改）；送 `CTRL_BREAK_EVENT`（需要 `CREATE_NEW_PROCESS_GROUP` 與主控台，Validation Required 的範圍更大）。
- 影響：Worker 在寬限期內沒有回報就收不到通知，只能被終止；Context Builder（票 39）與真實 Adapter（票 14）的 Worker 提示需要寫明「回應出現 `stop_requested` 時保存 checkpoint 並結束」。30 秒是否足夠屬 Validation Required。
- 審閱：未審

### AI-132：40 秒上限的實作與停止流程結束後的收尾
- 重要度：中
- 時間／階段：2026-10-22，票 25 實作
- 決策：全部 Run 的 `stop` 並行執行，寬限期 30 秒（ProcessControl 在之後 10 秒內終止並確認，AI-72 的既有行為），Runtime 另以 `寬限期 + 確認上限 = 40 秒` 的真實時間作為安全網：到時還沒回來的停止記為停止異常 `stop_unconfirmed`。停止期限（`deadline_at`）以 Clock 記在停止報告中。停止流程期間握住 Supervisor 的 tick 鎖。結束後 Runtime 再提供 API 5 秒（讓 App 讀到最終結果）才結束服務；測試 harness 不自行結束（`exit_linger: None`），以 `exit_timing` 縮短上限測試卡住的情境。
- 理由：spec D-11 的總上限是 40 秒；ProcessControl 的 `stop` 本身就有上限，但 Port 的實作可能卡住（例如 Windows API 沒有回應），Runtime 不能把有界性完全交給 Port。App 每 0.5 秒輪詢，5 秒足以讀到結果；Runtime 若永遠不結束，App 當掉時會留下無主的 Runtime。
- 替代方案：停止依序執行（N 個 Run 會變成 N × 40 秒）；Runtime 停止流程結束立即結束服務（App 讀不到停止異常）。
- 影響：從按下「完全退出」到 Runtime 結束最長約 45 秒；第 40 秒才回來的 `ForceKilled` 若剛好晚於安全網，會被記為 `stop_unconfirmed`（保守）。
- 審閱：未審

### AI-133：桌面殼：關窗隱藏到系統匣、完全退出由 Web UI 顯示進度，殼有 60 秒後備
- 重要度：中
- 時間／階段：2026-10-22，票 25 實作
- 決策：關閉主視窗改為隱藏（`prevent_close` + `hide`），App 留在系統匣；系統匣選單有停用的狀態項目（每 3 秒以 `GET /api/v1/lifecycle` 更新：運作中與進行中 Run 數、完全退出中、已停止、無法連線）、「開啟 Baton」（左鍵點圖示亦同）與「完全退出」。「完全退出」顯示主視窗並對 Web UI 發 `baton://full-exit-requested`；Web UI 呼叫 `lifecycle_exit`、每 0.5 秒輪詢並顯示每個 Run 的停止結果與期限倒數；沒有停止異常約 1 秒後自動 `app_quit`，有異常則列出並等使用者按「關閉 Baton」；輪詢時 Runtime 已結束也視為完成。要求退出本身失敗時顯示原因與「重試」「返回」，不自動結束 App。殼在按下後 60 秒若 App 仍未結束，自行送出完全退出（冪等）再結束 App。HTTP 版用戶端（瀏覽器開發用）的 `app_quit` 與事件監聽是 no-op。
- 理由：票面要求「系統匣提供狀態、重新開啟與完全退出」「顯示停止進度與異常」；有異常時自動關閉會讓使用者看不到「哪些程序可能還在背景」。後備避免 Web UI 卡住時 App 永遠不結束；在系統匣按「完全退出」本身就是停止意圖，所以後備也送出退出要求。
- 替代方案：殼在 Rust 端自己輪詢並以原生對話框顯示進度（與 Web UI 的狀態呈現重複）；有異常也自動結束 App（異常只留在下次開啟才看得到）。
- 影響：沒有單一執行個體機制，App 隱藏在系統匣時再執行一次執行檔會開出第二個 App（兩者連同一個 Runtime，不會有第二個 Runtime 或 Worker）；可在後續票加入 `tauri-plugin-single-instance`。系統匣與關窗行為只有編譯與元件測試，Windows 上的實際操作屬 Validation Required（已寫入 `docs/development.md`，建議列入票 51）。
- 審閱：未審

### AI-134：生命週期 API、資料表與事件的形狀
- 重要度：低
- 時間／階段：2026-10-22，票 25 實作
- 決策：端點：`GET /api/v1/lifecycle`（階段、`stop_intent_active`、時間參數、進行中或最近一次的停止報告、進行中 Run 數、各使用中 Workspace 的 `auto_resume_after_exit`／`paused_since`／`awaiting_resume`）、`POST /api/v1/lifecycle/exit`（202）、`PUT /api/v1/workspaces/{id}/lifecycle`（`{ auto_resume_after_exit }`）、`POST /api/v1/workspaces/{id}/resume`（停止意圖生效時 409 `stop_intent_active`）。停止報告的每個 Run：`outcome` = `pending`／`not_started`／`stopped_gracefully`／`force_killed`／`anomaly`，異常再帶 `anomaly` = `still_alive`／`stop_failed`／`stop_unconfirmed` 與說明。migration 14：`runtime_lifecycle`（單列）與 `workspace_lifecycle`（沒有列的 Workspace 等同預設：自動續跑關閉、未暫停）。事件：`full_exit_requested`、`full_exit_completed`（含異常清單）、`runtime_reopened`（含續跑與等待的 Workspace）、`workspace_paused_by_exit`、`workspace_resumed`、`workspace_auto_resume_changed`；停止進度的逐筆更新不追加事件。UI：Workspace 清單每列有「完全退出後自動續跑」勾選與暫停中的「繼續工作」，等待原因第七類標籤為「完全退出」。
- 理由：一個總覽端點同時供系統匣、退出進度與 Workspace 清單使用；進度更新很頻繁，事件只記開始、結束與重新開啟。
- 替代方案：把 Workspace 設定放進 `Workspace` 本身（要改 Workspace 狀態機、API 與既有測試）；停止進度用 SSE（事件串流目前還沒有實際事件，輪詢已足夠）。
- 影響：migration 若與平行開發的票衝突，依 AI-75／AI-86 的方式順延（整合時已順延為第 14 個，見 AI-135、AI-136）。「跨登入自動續跑」（使用者故事 17 的另一個設定）屬票 33，屆時加在同一張 `workspace_lifecycle` 表。
- 審閱：未審

### AI-135：票 25 整合 rebuild/v1（票 12、08）：migration 與 AI 編號順延
- 重要度：中
- 時間／階段：2026-10-22，票 25 實作（合併 rebuild/v1）
- 決策：(1) 整合分支已有 `artifact`（第 11 版）與 `shared_settings_version`（第 12 版），本票的 `lifecycle` migration 順延為第 13 版；三者沒有共用資料表。(2) 本票原本的 AI-103～AI-109 與整合分支的票 12、08 條目重號，順延為 AI-121～AI-127（內文沒有互相引用，只把 AI-127 的 migration 編號改為 13）；整合分支的條目沒有改動。(3) `baton-api`、`baton-client`、Tauri 殼、Web UI 與 `docs/development.md` 兩邊新增的端點、指令、型別、樣式與段落全部保留（生命週期與成果、共同設定並存）。
- 理由：已發布的 migration 不可修改，順延尚未合併的本票版本（與 AI-86、AI-117 相同做法）。
- 替代方案：把本票的 migration 插在前面（會改動整合分支已有的版本號，不可行）。
- 影響：開發中的本機資料庫若套用過本票舊版本號的 migration，需要重建。
- 審閱：未審

### AI-136：票 25 再次整合 rebuild/v1（票 27）：migration 第 14 版、AI 編號再順延
- 重要度：中
- 時間／階段：2026-10-22，票 25 修正 run R049（合併 rebuild/v1）
- 決策：(1) 整合分支已先合併票 27，其 migration `quota` 是第 13 版，本票的 `lifecycle` 順延為第 14 版（排在 `quota` 之後；兩者沒有共用資料表）。(2) 本票條目原本的 AI-121～AI-128 與票 27 的 AI-121～AI-127 重號，順延為 AI-128～AI-135，移到票 27 條目之後；條目內的交互引用（AI-127 中的「見 AI-128」→ AI-134 中的「見 AI-135、AI-136」）與 migration 編號（AI-134 的 13 → 14、`docs/development.md`）同步更新，記錄歷史編號的敘述（AI-135）保留原文。原 AI-127、AI-128 缺少的審閱行與空行在順延時補上。(3) 其餘衝突兩邊都保留：Supervisor 同時有生命週期（重新開啟、完全退出）與額度輪詢、Adapter 額度事件；`baton-api`、`baton-client`、`baton-domain`、`baton-store` 的模組與匯出、Tauri 指令（`app_quit` 與 `profile_set_quota_limits`）、開發指南的 crate 表與情境測試清單合併為同時描述兩張票。
- 理由：已進入整合分支的 migration 與條目編號不可改動，順延尚未合併的本票（與 AI-117、AI-119、AI-135 同）。
- 替代方案：把兩個 migration 合併成一個（會改動整合分支已有的第 13 版）。
- 影響：已用本票分支建過的開發資料庫，`lifecycle` 記在第 13 版，與整合後的編號不一致，需要刪除重建。
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
