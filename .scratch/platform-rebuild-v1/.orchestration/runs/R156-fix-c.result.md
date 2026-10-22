```
RESULT: DONE
COMMITS: 20b0fe5..f9e2c0a
TESTS:
- pnpm test → Rust：通過；前端：通過（30 個檔案、648 個測試）
- pnpm --filter web typecheck → 無錯誤
- cargo check -p baton-desktop（經 with-msvc.ps1）→ 通過（只有 baton-api 原本就有的 unused import 警告）
- cargo test -p baton-desktop → 9 passed
- cargo test -p baton-harness --no-fail-fast（BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw）→ 全部通過，真實瀏覽器測試有實際執行
- pnpm build，接著 node scripts/smoke-desktop.mjs → 冒煙測試通過（exit 0），沒有殘留的 baton-* 程序
FIXED:
- R153 第 1 項（中）— App.tsx 中 AuthorizationPanel、ModelOverridesPanel、ConversationPage、TaskBoard、DecisionCenter、ArtifactsPage、KnowledgePage 都加上 key={workspace.id}，切換 Workspace 時重新掛載 — App.test「切換 Workspace 後，上一個 Workspace 較晚回來的授權設定不會蓋掉目前的草稿」（81e542d）
- R153 第 2 項（中）— 殼端新增 FallbackTimer（可取消，已有計時器時不再另起）與 Tauri 指令 full_exit_cancel_fallback；Web UI 在 requestFullExit 成功或按「返回」時取消計時器 — fallback_timer.rs 5 個單元測試、ExitProgress.test 3 個測試（6b47cc3、432248b）
- R153 第 3 項（中）— 啟動狀態改用 StartupCell：只保留成功結果，失敗時下次呼叫會重試，並行呼叫只啟動一次 — startup_cell.rs 4 個單元測試（20b0fe5）
- R153 第 4 項（中）— UpdateProgress 只用「看到 ready 之後」讀到的 lifecycle 判斷是否自動安裝 — UpdateProgress.test「看到準備套用後讀不到停止結果時，不以停止中讀到的舊結果自動安裝」（544088a）
- R153 第 5 項（中）— updateStatus 連續失敗 3 次時，顯示「嘗試安裝更新」與「關閉 Baton」 — UpdateProgress.test 3 個測試（91bd1a8）
- R153 第 6 項（中）— ExitProgress 連續失敗 3 次才判定 runtime_gone，成功一次就歸零 — ExitProgress.test 2 個新測試，並加強既有測試（f472021）
- R153 第 7 項（中）— 狀態列顯示「可用帳號：N」（listProfiles 中 dispatchable 的數量） — StatusBar.test 2 個測試（160b4b0）
- R153 第 8 項（中，部分）— 冒煙腳本加入「恢復事件顯示」（強制結束 Runtime，由監護拉起）與「完全退出進度」（在頁面內以 Tauri IPC 發出事件），實跑通過（abaae8d）
- R153 第 9 項（低）— finally 一定會結束 app1、app2、監護與 Runtime；Runtime PID 也從 guardian\runtime.json 取得 — 以「啟動後 15 秒丟錯誤」的副本驗證沒有殘留（abaae8d）
- R153 第 10 項（低）— 設定了 BATON_PLAYWRIGHT_DIR 時 Playwright 不可用就判為失敗；沒設定時才略過，docs 已註明 — web_verification_real_browser.rs（1924483）
- R153 第 11 項（低）— 5 處真實 sleep 改為等待 Notify 或事件（上限 10 秒） — lifecycle、worktrees、claude_adapter 三個測試檔各連跑 7 次以上，都通過（c3ff100）
- review-notes #2（App.test 部分）— 根因是第一個測試要承擔 App 首次 render 的冷啟動成本（實測 250～1158ms）；改為 beforeAll 先 render 一次暖機，並在 beforeEach 清除呼叫紀錄，斷言的逾時都沒有拉長（accdecc）
- review-notes #23 — 移除 dead_code 的 error_code；補上對 /decisions 清單的斷言（superseded 與 pending_count）（6ea3e3d）
- review-notes #28（高）— 根因：Harness 被丟棄時，Runtime 的背景工作仍開著 SQLite 與日誌，TempDir 刪除失敗又不報錯。改為：資料目錄放在 %TEMP%\baton-harness，搭配獨占鎖檔，交給背景執行緒重試刪除；程序先結束時，由之後啟動的 harness 清掉鎖檔已無人持有的目錄 — harness_cleanup.rs 4 個測試（修正前 2 個失敗）；在專用 TEMP 下跑完整 harness，沒有留下任何 .tmp*（f1f8b19）
NOT_FIXED:
- R153 第 8 項的「派工到完成」「裁定中心批次處理」UI E2E 沒有做。裁定請求只能用 Agent 憑證建立，真實 Runtime 又沒有 fake Adapter，這兩條旅程都需要真的 Worker／Controller 執行；要做的話得另開票，設計使用 fake Port 的 E2E 組態（AI-455）
NOTES:
- 沒有跨範圍修改。ai-decisions.md 新增 AI-449～AI-457。另外兩個修正 run 若也追加條目，合併時可能撞號，需依 AI-75／AI-109 順延。
- 需要驗證者特別看的地方：
  - AI-450：沒有照審查建議「向殼查詢 ready_package」。殼只有在 UI 輪詢時才會記下 ready_package，所以多加一個查詢指令也救不回任何情況；改為直接讓使用者嘗試安裝。
  - AI-451：requestFullExit 成功後也會取消殼端的後備計時器。之後若 Web UI 卡住，就不會再有自動關閉 App 的後備。
  - AI-452：並行等待的呼叫者共享同一次失敗結果，不會各自重新啟動。
- 本範圍外但看到的問題：
  - crates/baton-runtime/tests/browser_driver.rs 也有「沒有 Playwright 時印出訊息就 return，結果顯示為通過」的問題，同第 10 項。
  - review-notes #23 中「只在重新整理時才判定過期」的路徑，仍只有 domain 單元測試涵蓋。
  - %TEMP% 根目錄已累積的約 4.8 萬個 .tmp*（修正前留下，另有其他 run 正在產生）不會被自動清除，需在沒有測試執行時手動刪除。
- 冒煙腳本會在桌面開視窗，並以頁面內事件觸發完全退出；系統匣選單本身和殼端的 60 秒後備流程沒有自動化涵蓋。
```
