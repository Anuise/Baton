驗證結論：PASS。三項修正都確實修好；把修正暫時改壞時，新測試都會失敗；完整的 `pnpm test` 結束碼為 0，專用 TEMP 沒有測試留下的資料。

```
VERDICT: PASS
CHECKED:
- [PASS] #29 Web 確認授權帶 expected_revision — 授權設定頁（AuthorizationPanel.tsx:160）和裁定中心（DecisionCenter.tsx:843）都帶確認摘要的 changes.base_revision，第一次確認送 null。HTTP 版送 expected_revision；Tauri 版送 expectedRevision，對應的 src-tauri/src/main.rs:366 本來就有接 expected_revision: Option<i64>，會轉給 Runtime。Web 端只有這兩處呼叫 confirmAuthorization，而且欄位是必填，之後新增的呼叫端漏帶會在型別檢查時報錯。
- [PASS] #29 測試能抓到缺陷（暫時改壞再還原）— 把三處都改成送 null 後，有 4 個測試失敗：runtimeClient 的 HTTP 案例、AuthorizationPanel 的「擴大範圍」和「另一個視窗先確認後被拒絕」、DecisionCenter 的「修改 Workspace 授權」。把 harness 測試 when_two_windows_… 改成送 null 後也失敗：後送出者得到 201，並寫入 revision 3（會覆蓋對方的授權），正確應為 409 authorization_revision_conflict。兩處都已用 git checkout 還原，git status 乾淨。
- [PASS] #29 MCP shim 工具說明（AI-450）— scenario.rs 新增的斷言檢查命令與 {{secret:…}} 的操作必須和 Profile 完全相同、網址來源必須在 Profile 出現過、使用者資訊或佔位字、exploration_invalid。說明本來就在 merge 1e8f815 寫入，這次補上斷言，並在 pnpm test 的 Rust 步驟通過。docs/development.md 的「重現」段落已同步。
- [PASS] #30 clippy 是修程式碼清掉的 — 改動都不改變行為：&[x.clone()] 改成 std::slice::from_ref、filter().last() 改成 rfind、len()+1 <= n 改成 len() < n（整數下等價）、改用 enumerate、新增型別別名 ProbeHook／Case。新增的 #[allow] 只有 3 個，都加在函式上、都是 too_many_arguments、都附繁體中文註解（Supervisor::new、workflow::skip_stage、Store::skip_stage），全 repo 沒有 crate 層級的 #![allow]。斷言都沒有被刪除或弱化。cargo clippy --workspace --all-targets -- -D warnings 的結果是 0 個 warning／error，結束碼 0。
- [PASS] #30 pnpm test 會跑 clippy — scripts/test.mjs 第一步執行 clippy -D warnings，實際輸出有「Clippy：通過」。CI 的工具鏈加了 components: clippy。我另外確認 CI 的 pnpm test 排在 pnpm build 之前不會出問題：清掉 baton-desktop 的建置產物並暫時移開 web/dist 後，clippy -p baton-desktop 仍然通過（結束碼 0），web/dist 已放回。
- [PASS] #23 未使用的 StoreError／error_code — 已在 rebuild/v1 修掉：shared_settings.rs 裡已沒有 StoreError，gate_invalidation.rs 裡已沒有 error_code。clippy -D warnings 的結果為 0，也證明沒有未使用的 import 或變數。
- [PASS] NOT_FIXED 為「無」。AI-511、AI-512 都已記錄；AI-511 寫明殘留限制：兩個視窗同時做「第一次確認」時擋不住後送出者，要改 Domain 語意才能擋。這個限制合理，已有對應決策。
- [PASS] 專用 TEMP 沒有殘留 — TEMP／TMP 指向 %LOCALAPPDATA%\Temp\r169-verify-d。跑完後只剩空的 baton-harness（0 個項目，AI-508 允許）和 Node 24 的 node-compile-cache（49 個 Node 自己的快取檔，不是測試產生的）。這個目錄已刪除，也確認不存在了。
- [PASS] 前置條件 — rebuild/v1 仍停在 e6f4063，已併入 HEAD 1882582，驗證期間沒有前進。驗證結束時工作樹乾淨。
TESTS:
- pnpm test（TEMP／TMP 用專用目錄，BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw）→ 結束碼 0，耗時 4 分 19 秒。Clippy：通過；Rust：通過；前端：通過；UI E2E：通過，以真實瀏覽器跑了 3 個測試，不是略過。
- pnpm --filter web typecheck → 0
- cargo check -p baton-desktop → 0
- cargo clippy --workspace --all-targets -- -D warnings → 0，沒有任何警告
- vitest 只跑 AuthorizationPanel／DecisionCenter／runtimeClient：未改動時 158 個全部通過；把修正改壞後 4 個失敗。
- cargo test -p baton-harness --test workspace_authorization when_two_windows：把測試改壞後失敗（201 而非 409）；已還原，完整 pnpm test 中通過。
FIXES_NEEDED: none
```
