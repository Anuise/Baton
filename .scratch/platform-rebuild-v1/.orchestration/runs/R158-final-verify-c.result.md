VERDICT: FAIL

只有一個項目沒過：review-notes #28（高）。R153 的 7 個「中」項目都確認修好，新測試在修正前的程式碼上都會失敗；三個指定指令也都通過。問題在於每跑完一次 harness 測試，`%TEMP%` 仍會留下 1 個完整的 Baton 資料目錄，而且 #28 要求的「檢查其他測試」沒有做。

CHECKED:
- [PASS] R153-1（中）切換 Workspace 時過期回應蓋掉授權草稿 — `App.tsx` 七個元件都加了 `key={opened.workspace.id}`。我把 web 複製到暫存目錄，原始碼換回 `rebuild/v1` 版本再跑新測試，App.test 新增的那個測試失敗，失敗點是「讀取檔案」沒有勾選，正好重現這個 bug。修正後通過。
- [PASS] R153-2（中）60 秒後備計時器無法取消 — 新增可取消的 `FallbackTimer`，已有等待中的計時器時不再另起；新指令 `full_exit_cancel_fallback` 已註冊。Web UI 在 Runtime 接手或按「返回」時會取消。ExitProgress 的 2 個新測試在修正前失敗；`fallback_timer` 的 5 個單元測試通過。
- [PASS] R153-3（中）啟動失敗被永久快取 — 改用只保留成功結果的 `StartupCell`，4 個單元測試通過。我也確認了重試不會重複啟動：`ensure_runtime` 會先探測 health，Runtime 本身有單一實例保護。AI-452 有說明「並行等待者共享同一次失敗」。
- [PASS] R153-4（中）依過時的停止結果自動安裝 — 改用 `finalLifecycle` 與 `readySeen` 判斷，新測試在修正前失敗。
- [PASS] R153-5（中）Runtime 提前結束後畫面卡在停止中 — 連續 3 次讀不到維護就提供「嘗試安裝更新」與「關閉 Baton」，3 個新測試在修正前失敗。AI-450 沒有照審查建議新增查詢指令，理由我核對過成立：殼只在 `update_status`／`update_install` 讀到維護狀態時才記下 `ready_package`（`main.rs:875,896`）。
- [PASS] R153-6（中）lifecycle 失敗一次就判定 Runtime 已結束 — 改成連續 3 次失敗才判定，成功一次就歸零；新測試在修正前失敗，舊測試也加強了。
- [PASS] R153-7（中）狀態列缺「可用帳號數」 — 以 `listProfiles` 中 `dispatchable` 的數量顯示，新測試在修正前失敗；取用來源記在 AI-449。
- [PASS] R153-8（中，部分）NOT_FIXED 的「派工到完成」「裁定中心批次處理」UI E2E — 理由合理（真實 Runtime 沒有 fake Adapter，裁定請求只能用 Agent 憑證建立），也有 AI-455。但 spec Testing Decisions 3 沒有註記延後，也沒有開票，和原審查指出的「寫了後續票卻沒人承接」是同一個模式。這項不擋，建議主控開票。冒煙腳本新增的兩條旅程我只核對了程式碼，沒有實跑。
- [PASS] R153-9～11（低）— 冒煙腳本失敗時一定會結束 App1、App2、監護與 Runtime；設定了 `BATON_PLAYWRIGHT_DIR` 時真實瀏覽器測試不可略過，這次它有實際跑（`ok`，沒有略過訊息）；5 處真實 sleep 都改為等待 Notify 或事件，上限 10 秒。
- [PASS] review-notes #2（App.test 部分）與 #23 — 只有暖機的 `beforeAll` 用 10 秒逾時，斷言的逾時都沒拉長；`gate_invalidation` 補上 superseded 與 pending_count 的斷言，`error_code` 已移除。
- [PASS] 沒有刪除或弱化既有測試 — diff 中被移除的測試宣告只有兩個，都是改名後加強；被移除的唯一一行 assert/expect 是 `lib.rs` 改用 `DataDir` 的那行，不是測試斷言。
- [FAIL] review-notes #28（高）harness 遺留暫存資料目錄 — 大量累積確實修好了：`harness_cleanup.rs` 在修正前 4 個中有 3 個失敗（修正回報寫 2 個，有出入但不影響結論）；在專用 TEMP 連跑兩次 `cargo test -p baton-harness`，`%TEMP%` 根目錄沒有新增 `.tmp*`。但兩次跑完都剩下 1 個完整的 Baton 資料目錄，例如 `%TEMP%\baton-harness\hz6thQA`，裡面有 `control.sqlite`、`profiles`、`logs`、`guardian`。這個目錄要等下一次執行才會被清掉，不符合「跑完不應新增殘留」的要求。另外，跑完整 `pnpm test` 時 `%TEMP%` 根目錄留下 2 個 `.tmp*`，會一直累積：
  - 一個來自 `baton-runtime/tests/supervisor_process_tree.rs`，內含 `work\.baton`；
  - 另一個是空目錄，可能來自 `command_runner` 或 `browser_driver`。我逐一單獨執行時，這兩個測試每次都留下空的 `.tmp*`，而這次單獨跑時 `supervisor_process_tree` 沒有留下，可見與時間有關。

  #28 明確要求檢查其他測試，AI-457 卻把它劃為範圍外。
- [PASS] 沒有引入阻擋性的新缺陷 — 有一個低風險的觀察：Web UI 接手完全退出後，如果使用者又按一次系統匣的「完全退出」，殼會重新排定 60 秒計時器，但 Web UI 會忽略重複的事件、不會取消它，所以 60 秒後即使畫面停在停止異常清單也會被關掉。這是使用者自己再按一次退出，不列為必修。
- 前置條件（不影響判定）：驗證途中 `rebuild/v1` 從 `f667321` 前進到 `2992715`（修正 A 已合併）。修正 A 使用了 AI-449～AI-473，和本分支的 AI-449～AI-457 撞號；它也改了 `src-tauri/src/main.rs`。合併時要依 AI-75／AI-109 順延編號，並注意衝突。

TESTS:
- `pnpm test`（專用 TEMP，`BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw`）→ exit 0。Rust 1661 個通過、0 個失敗、8 個 ignored；前端 30 個檔案、648 個測試通過。你提供的路徑 `Tempaton-pw` 應該是 `\b` 被轉義吃掉了，我用的是實際存在的 `Temp\baton-pw`。
- `pnpm --filter web typecheck` → exit 0。
- `cargo check -p baton-desktop`（經 `with-msvc.ps1`）→ exit 0，只有 `baton-api` 原本就有的 `StoreError` unused import 警告。
- 額外跑的：
  - `cargo test -p baton-desktop` → 9 個通過；
  - `cargo test -p baton-harness` 跑兩次 → 每次 608 個通過；
  - 修正前的程式碼配新測試：web 新增的 11 個測試全部失敗；`harness_cleanup.rs` 4 個中有 3 個失敗。
- 沒有跑 `smoke-desktop.mjs`：它會在桌面開視窗，也不在指定範圍內。
- 我建立的暫存目錄（包括 node_modules junction，已先單獨移除）都已刪除。工作樹維持乾淨，`web/node_modules` 完好。

FIXES_NEEDED:
1. review-notes #28：每個 harness 測試程序結束時，最後一個 harness 的資料目錄也要刪掉。
   - 現況：背景刪除執行緒在程序結束時被直接中止，只能等下一次執行清理。
   - 做法：在 `crates/baton-harness/src/data_dir.rs` 把待刪目錄記在全域清單，並註冊程序結束時的掛勾（例如 `libc::atexit` 或 `ctor` 的 `#[dtor]`）。這時各測試的 tokio runtime 都已結束、檔案都已關閉，可以同步重試刪除（設上限，例如 10 秒）。
   - 測試：補一個測試，以子程序執行一個使用 harness 的測試，等子程序結束後斷言 `%TEMP%\baton-harness` 下沒有它的 `h*` 目錄或 `.lock`。
   - 驗收：在專用 TEMP 跑完 `cargo test -p baton-harness` 後，`baton-harness\` 是空的。
2. review-notes #28 的「檢查其他測試」：
   - 修正 `baton-runtime` 測試留在 `%TEMP%` 根目錄的 `.tmp*`：`tests/supervisor_process_tree.rs`（會留下 `work\.baton`）、`tests/command_runner.rs`、`tests/browser_driver.rs`（留下空目錄，推測是子程序結束前就刪除 TempDir，或工作目錄還被佔用）。
   - 做法：在 TempDir 丟棄前確實等子程序或 Runtime 結束，或改用可清理的機制。
   - 同步修改 AI-457「其他 crate 不在範圍內」的說法。
   - 驗收：在專用 TEMP 跑完整 `pnpm test` 後，TEMP 下沒有任何 `.tmp*`，也沒有 Baton 資料目錄。
3. （非必修，建議主控處理）為 spec Testing Decisions 3 剩下的兩條 UI E2E 開票（設計使用 fake Port 的 E2E 組態），或在 spec 註記延後，避免像 AI-31 一樣再度沒人承接。
