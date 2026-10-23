```
VERDICT: PASS
CRITERIA:
- [PASS] 頂層三個頁簽；主區任何時候只顯示一個功能，設定不再全部往下疊 — 頂層頁簽在 web/src/App.tsx:55-59（Workspace／帳號／設定）。主區依 shell.top 擇一渲染（App.tsx:279-321），Workspace 功能由 switch 擇一顯示（App.tsx:349-393）。對照 cli/v1 舊 App：舊版把精靈、帳號、排程、跨登入、更新、共同設定、備份、恢復、平台狀態全部疊在主畫面，新版已拆開。測試在 App.test.tsx:159、173、191、208 與 it.each「…且只顯示這個功能」。
- [PASS] Workspace 下有 Controller、工作、裁定、成果、審查與驗證（含審查／驗證／探索小頁簽）、知識（知識＋專案理解）、流程、時間線、授權（授權＋模型覆寫） — WORKSPACE_TABS 在 App.tsx:61-72，第十個「對話」照票面暫留。QUALITY_TABS 在 App.tsx:74-78，小頁簽在 App.tsx:358-369。知識頁疊放 Knowledge 與 ProjectUnderstanding（App.tsx:370-377），授權頁疊放 Authorization 與 ModelOverrides（App.tsx:384-390）。Controller 頁目前是佔位內容（App.tsx:171-179）。測試在 App.test.tsx:208、268、281、292。
- [PASS] 帳號頁簽含帳號、排程、共同設定；設定頁簽含跨登入啟動、更新、備份、恢復、平台狀態 — 帳號頁簽在 App.tsx:304-311（另把首次精靈放在最上方，見 AI-10），設定頁簽在 App.tsx:312-320。測試 App.test.tsx:173、191 也確認兩邊互不出現對方的內容。
- [PASS] 裁定頁簽與左側 Workspace 名稱旁顯示待裁定數徽章，回答裁定後更新 — 頁簽徽章在 App.tsx:150-154、237-239，側欄徽章在 WorkspaceSidebar.tsx 的 workspace-heading 區塊（「待裁定 N」）。回答裁定後由 onDecisionsChange 遞增 pendingVersion，觸發 StatusBar 立即重讀（App.tsx:299、243）。測試 App.test.tsx:369 驗證回答後兩處徽章都消失。
- [PASS] 切換 Workspace 時停在同一頁簽；新選 Workspace 預設 Controller；未選時顯示提示文字 — selectWorkspace 在 App.tsx:224-230，提示文字在 App.tsx:280-282。「新選」的解讀記為 AI-11。測試在 App.test.tsx:208、314，封存後回到提示在 :330。
- [PASS] 重新開啟 App 回到上次的頂層頁簽、Workspace 與頁簽；本機儲存讀不到時用預設值、不報錯 — loadShell／saveShell 在 App.tsx:96-117，以 try/catch 處理，並逐欄驗證頁簽名稱。測試涵蓋：回到上次位置（App.test.tsx:388、402）、內容讀不懂（:414）、頁簽名稱不認得（:422）、getItem／setItem 丟例外（:430）、上次的 Workspace 已不在清單（:449）。
- [PASS] 沒有可用 Account Profile 時首次精靈以全畫面顯示；狀態列、更新通知、完全退出進度不受頁簽影響 — 全畫面判斷在 App.tsx:208-222、250-258。StatusBar、UpdateNotice、ExitProgress 都在頁簽之外（App.tsx:243、248、327）。測試在 App.test.tsx:458、474、491、503、513、523。另外多了一顆「先進入主畫面」按鈕，可在這次開啟中跳過全畫面，已記為 AI-10（重要度中）。
- [PASS] 既有各頁面的元件測試仍全部通過；新增 App 外殼的元件測試涵蓋上述行為 — Vitest 結果為 Test Files 30 passed (30)、Tests 656 passed (656)。這次 diff 沒有改動其他任何元件檔或它們的測試檔，只改 App、WorkspaceSidebar、styles。WorkspaceSidebar.test 刪掉的測試都是針對已移除的功能按鈕，另補了選取、aria-current、onWorkspaces 的測試。
- [PASS] AI 決策已追加至 ai-decisions.md — 新增 AI-10（中）、AI-11（低）、AI-12（低），都附重要度與「審閱：未審」，只有追加、沒有改到既有條目（git diff 內容全是 + 行）。
- [PASS]（前置）cli/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor cli/v1 HEAD` 的結果是 ANCESTOR_OK。
TESTS:
- `pnpm --filter web build`（tsc --noEmit && vite build）→ 成功。
- `pnpm test`（node scripts/test.mjs）→ EXIT=0：Clippy 通過、Rust 通過、前端 30 個測試檔 656 個測試全數通過、UI E2E 3 passed（含改寫後的 dispatch_to_completion 與 decision_center_batch）。
- `pnpm build`（release 版 Runtime／Guardian 加 tauri build --no-bundle）→ EXIT=0，產出 target\release\baton-desktop.exe，約 2 分 19 秒。
- `node --check scripts/smoke-desktop.mjs` → 語法正確。冒煙腳本本身沒有執行，因為它要真實桌面環境與 WebView2 偵錯埠。
- 執行前後 `git status --short` 都是乾淨的。
FIXES_NEEDED: none
```

以下兩點不影響這次判定，留給後續參考：
1. 啟動時會先短暫閃過頁簽畫面，`getSetup` 回來後才切成全畫面精靈。上次選的 Workspace 在側欄清單載入前，也會短暫顯示「請從左側選擇或建立 Workspace」。兩者都只是初次載入時一閃而過的畫面。
2. 「先進入主畫面」這顆按鈕是 AI-10 自行加的，跟 spec 使用者故事 14「先完成必要設定才進入一般畫面」有點衝突，建議使用者審 AI-10 時一起確認要不要保留。
exit: 0

