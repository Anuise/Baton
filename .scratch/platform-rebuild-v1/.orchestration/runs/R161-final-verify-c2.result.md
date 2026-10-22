VERDICT: PASS

R158 的 FIXES_NEEDED 第 1、2 項都已修好。跑完整 `pnpm test` 後，專用 TEMP 裡只剩一個空的 `baton-harness\`：沒有 `.tmp*`，也沒有任何 Baton 資料目錄。三個指定指令都通過。

CHECKED:
- [PASS] R158 第 1 項（review-notes #28，高）程序結束時，最後一個 harness 的資料目錄也要刪掉 — `data_dir.rs` 新增全域待刪清單 `PENDING`。第一次有目錄進入清單時，用 `libc::atexit` 註冊 `remove_pending_at_exit`，在程序結束時同步重試刪除，上限 10 秒。atexit 內不會 panic：lock 中毒時改用 `PoisonError::into_inner`，也沒有 unwrap。驗證方式如下：
  - 用 `git archive cbbcfcf`（R159 修正前）解出原始碼，只換上新版的 `harness_cleanup.rs`，在獨立的 TEMP 與 target 執行。`nothing_is_left_after_the_test_process_exits` **失敗**：`子程序結束後仍留下：["hOOx5Kx", "hOOx5Kx.lock"]`。
  - 修正後在完整 `pnpm test` 中這個測試 `ok`，結束後 `baton-harness\` 是空的。
- [PASS] R158 第 2 項（#28「檢查其他測試」）— 新增 `crates/baton-runtime/tests/support/mod.rs` 的 `remove_temp_dir`，在結尾有上限地重試刪除，刪不掉就讓測試失敗。用到它的是 `command_runner` 的逾時測試、`browser_driver` 的兩個測試（包含略過的路徑），以及 `supervisor_process_tree`。這一項的修正本身就在測試碼裡，所以改用「殘留數」核對：
  - 修正前的原始碼，在專用 TEMP 跑這三個測試檔 3 次，每次都留 1 個 `.tmp*`（`.tmpL68xvY`、`.tmpUGsnpf`、`.tmphbHKcg`）。
  - HEAD 跑 4 次，0 殘留。
  - 全 repo 用到 `keep()` 的只有這兩處新程式，都是刻意的。AI-483 已刪掉「其他 crate 不在範圍內」的說法，改為指向 AI-484。
- [PASS] NOT_FIXED 的理由
  - 第 3 項 UI E2E 開票：整合分支已有票 53／AI-474，合理。
  - 產品端的 `LocalCommandRunner`／`LocalBrowserDriver` 沒有等被終止的子程序結束：R159 只被允許改測試，AI-484（低）有寫明理由、替代方案與影響，合理。
- [PASS] AI 決策檔
  - 本分支相對於 `rebuild/v1` 的 diff 中，`ai-decisions.md` 沒有任何刪除行。
  - 在本分支內，AI 編號沒有重複。
  - `docs/development.md` 的引用改成 AI-481／AI-484，內容和決策一致。
- [PASS] 沒有刪除或弱化既有測試 — 被移除的 assert／expect／it 只有三行：`lib.rs` 改用 `DataDir` 的那行，以及 R158 已核對過的兩個改名後加強的 web 測試。R159 只在測試結尾加清理呼叫，沒有改動任何斷言。
- [PASS] 沒有引入新缺陷 — 只有一個低風險的觀察：每次 Drop 都會另開一條背景執行緒，而且要等整個 `PENDING` 清空才結束，所以同一個程序裡可能有多條執行緒同時退避重試。上限是 10 分鐘，不會擋住程序結束，完整測試也沒有變慢。不列為必修。
- [注意，不影響判定] 前置條件：驗證途中 `rebuild/v1` 從 `ecbbc65` 前進到 `2af5614`（已合併修正 B），HEAD 不再包含整合分支最新的提交。合併時要注意三件事：
  - 修正 B 已佔用 AI-475～AI-491，本分支的 AI-475～AI-484 又會撞號，要再順延，並同步更新 `docs/development.md` 的 AI-481／AI-484。
  - 修正 B 也改了 `crates/baton-runtime/tests/{browser_driver,command_runner,supervisor_process_tree}.rs`、`baton-harness/src/fakes.rs` 與 `docs/development.md`，可能有衝突。
  - 修正 B 的 71fd552 讓驗證命令改用 Job Object 並等命令結束，AI-484「產品端不等子程序結束」的影響說明可能需要依此更新。

TESTS:
- 完整 `pnpm test`：用專用 TEMP `%LOCALAPPDATA%\Temp\baton-verify-c2`，並設定 `BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw`。
  - 你給的 `Tempaton-pw` 應該是 `\b` 被轉義吃掉了，我用的是實際存在的 `Temp\baton-pw`。
  - 結果 exit 0。Rust 1709 個通過、0 個失敗、9 個 ignored；前端 30 個檔案、648 個測試通過。
  - 真實瀏覽器測試 `a_session_drives_a_real_browser_and_returns_its_trace_and_video` 有實際執行，結果 `ok`。
  - 結束後專用 TEMP 只剩空的 `baton-harness\`。真實 `%TEMP%` 根目錄在這段期間也沒有新增 `.tmp*`。
- `pnpm --filter web typecheck` → exit 0。
- `cargo check -p baton-desktop`（經 `with-msvc.ps1`）→ exit 0，只有 `baton-api` 原本就有的 `StoreError` unused import 警告。
- 修正前的對照：
  - `cbbcfcf` 原始碼配新版 `harness_cleanup.rs`：1 個失敗、4 個通過。
  - `baton-runtime` 的三個程序測試，修正前 3 次各留 1 個 `.tmp*`，修正後 4 次都是 0。
- 我建立的暫存目錄、target 與記錄檔都已刪除，工作樹維持乾淨。

FIXES_NEEDED: none
