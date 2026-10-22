```
RESULT: DONE
COMMITS: ee64f68..b77a737
TESTS:
- node scripts/test.mjs（= pnpm test；TEMP/TMP 指向專用的 %LOCALAPPDATA%\Temp\r168-fix-d-test，BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw）→ 結束碼 0。Clippy：通過；Rust：通過（129 組測試結果，0 失敗）；前端：通過（30 個檔案、650 個測試）；UI E2E：通過（實際以 baton-pw 的 Playwright 執行）。
- 跑完後專用 TEMP 只剩兩個目錄：harness 共用的空 baton-harness（0 個項目，AI-508 允許）和 Node 24 自己的 node-compile-cache（Node 的編譯快取，不是測試留下的資料）。專用 TEMP 已刪除，也確認不存在了。
- cargo clippy --workspace --all-targets -- -D warnings → 0 個錯誤（含 baton-desktop 和測試目標）
- pnpm --filter web typecheck → 0
- cargo check -p baton-desktop → 0
- rebuild/v1 沒有新 commit，不需要 merge。工作樹乾淨。
FIXED:
- review-notes #29（AI-471）— Web 的 ConfirmAuthorizationRequest 新增必填的 expectedRevision。HTTP 版送 expected_revision，Tauri 版送 expectedRevision。授權設定頁和裁定中心「修改 Workspace 授權」都帶確認摘要的 changes.base_revision；第一次確認沒有 changes，送 null — 測試：runtimeClient.test.ts 的 HTTP 和 Tauri 兩個案例；AuthorizationPanel.test.tsx 新增「另一個視窗先確認後，這次送出被拒絕，不覆蓋對方的授權」，fake 的檢查規則和 Runtime 相同；DecisionCenter.test.tsx 斷言 expectedRevision: 3。這些都是先失敗、改完後才通過。另補 harness 情境測試 workspace_authorization.rs::when_two_windows_edit_concurrently_the_later_confirmation_is_refused_without_overwriting，證明後送出者收到 409 authorization_revision_conflict、生效的仍是 revision 2，而且沒有寫入第三筆事件。
- review-notes #29（R151 第 3 項／AI-450）— reproduce_exploration_finding 的工具說明其實已在 merge 1e8f815 寫進 Profile 限制（命令、{{secret:…}}、網址來源、使用者資訊或佔位字、exploration_invalid），只是工具清單測試沒有斷言。已在 mcp-shim tests/scenario.rs 補上斷言，docs/development.md 的「重現」段落也補了同一組限制 — 測試：cargo test -p baton-mcp-shim --test scenario
- review-notes #30 — 清掉全部 clippy 警告，用 std::slice::from_ref、rfind、enumerate 改寫，複雜型別改用型別別名（ProbeHook、Case），supervisor 模組說明的「5½.」改為縮排續行。只有 Supervisor::new、workflow::skip_stage、Store::skip_stage 三個函式各加一個 #[allow(clippy::too_many_arguments)]，附繁體中文註解說明理由（AI-512）。scripts/test.mjs 第一步改跑 clippy -D warnings，CI 的 rust-toolchain 加上 components: clippy，docs/development.md 同步更新 — 測試：clippy 為 0，pnpm test 全過
- review-notes #23／#9（StoreError、error_code）— rebuild/v1 已經修掉了（7a31ac6、6ea3e3d），本 run 不需要再改。清理 clippy 後也沒有任何編譯警告。
NOT_FIXED: 無
NOTES:
- 決策編號用了 AI-511、AI-512，接在 AI-510 之後。如果同時進行的其他修正 run 也用了這兩個號碼，合併時要依 AI-506／AI-510 的做法順延。
- AI-511 的殘留限制：兩個視窗同時對「尚未確認授權」的 Workspace 做第一次確認時，因為沒有 base revision，後送出者仍會建立 revision 2。要擋住這種情況得改 Domain 的語意，本 run 沒有做。
- harness 並行情境測試一寫好就通過，因為後端早已支援 expected_revision（AI-471）。它的用途是端到端證明「帶上摘要的 base_revision 就會擋下後送出者」。真正驗證 Web 修正的是那些先失敗、改完才通過的前端測試。
- 跨範圍修改：clippy 修正碰到 baton-domain、baton-store、baton-runtime、baton-harness 的多個檔案（多數是測試），全部是不改行為的機械式改寫；另外動了 .github/workflows/ci.yml。
- CI 用的是 stable 工具鏈，Rust 新版本新增的 lint 可能讓 CI 變紅（AI-512 的影響欄已註明）。
- worktree 原本沒有 node_modules，本 run 執行了 pnpm install --frozen-lockfile；這是 gitignore 的目錄，不影響工作樹。
```
