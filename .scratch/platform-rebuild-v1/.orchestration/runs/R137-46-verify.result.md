## 驗證結果（R137-46-verify）

```
VERDICT: FAIL
CRITERIA:
- [PASS] 前置：rebuild/v1 是 HEAD 的祖先 — `git merge-base --is-ancestor rebuild/v1 HEAD` → ANCESTOR_OK（rebuild/v1 = 5bd8a7b，已併入 c769688）
- [PASS] 模型限流不被記為產品缺陷 — 判定邏輯：crates/baton-domain/src/verification_profile.rs:1326-1328 把 RateLimited 判為 inconclusive／judge_rate_limited／model:rate_limited；:1441-1442 只有在沒有其他明確不符時才採用這個未判定原因。限流後同一次驗證不再呼叫模型：crates/baton-runtime/src/verification_adapter.rs:203-222。正式 judge 的 stream-json 解析在 crates/baton-runtime/src/semantic_judge.rs:94-130，單元測試涵蓋 rejected 事件、用量上限結果與 allowed_warning。情境測試：crates/baton-harness/tests/web_verification_adapter.rs:290-354，涵蓋限流、無法使用、預算用盡、沒有帳號、限流時程式斷言仍不符，都通過。（小問題，不影響本條：`usage_limit_reset` 的關鍵字比對不到 "rate_limit_error" 這類 API 429 文字，那種情況會判成 judge_unavailable，仍是 inconclusive，不會變成產品缺陷。）
- [FAIL] 秘密不出現在 Prompt 或證據中 — Prompt 這半條通過：judge 的網址、標題、畫面文字都已遮蔽（verification_adapter.rs:209-215），Bundle 文字也已遮蔽（judge_step／not_run 都經過 sanitize）。證據檔案這半條不通過：畫面「顯示」秘密時，截圖能不能保存只看 `vp::withhold_reason` 在檔案位元組裡找不找得到秘密字串（verification_profile.rs:1641-1653，由 verification_adapter.rs:411 呼叫）。真實 Runner 交回的是 Playwright 的 PNG（runner.mjs:113），畫面上的文字只是像素，在位元組裡找不到，所以截圖一定會被保存。情境測試 web_verification_adapter.rs:413-453 斷言「登入後把密碼印在畫面上」的截圖是 `secret_detected`，但能通過只是因為 fake 的截圖內容就是明文「PNG <url>\n<畫面文字>」（crates/baton-harness/src/fakes.rs:1768-1771），和正式實作不一致。同理，沒有「輸入」過秘密、但畫面「顯示」過秘密的 Session，它的 trace（壓縮過的 zip）與錄影也會被保存。換句話說，到了正式環境，秘密會以影像形式進入證據檔案。
- [PASS] 測試 persona 與實際登入身分分開綁定 — 情境測試 web_verification_adapter.rs:502-578：persona 的 key 不能拿來當 identity（發布時 422）；identity 沒有自動 check 時 422；Bundle 的旅程結果分別記下 persona 與 identity；階段依序是 login×4 → identity → journey；persona 是 admin 但綁店員帳號時判為 auth_blocked／http_forbidden；帳密實際登入成店員時判為 identity_not_confirmed，旅程步驟 blocked_by_identity 不執行，也不呼叫 judge；登入失敗時判為 login_failed。每條旅程開一個 Session，結束後全部關閉（:535-539）。
- [FAIL] 以主要 seam 撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加 — ai-decisions.md 已有 AI-408～AI-414，情境測試也全部通過；但上一條的證據檔案遮蔽，情境測試是靠 fake 截圖的明文格式才通過，並沒有驗到正式行為（fake 與正式實作分歧），所以這一條只算部分成立。
TESTS:
- `BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw node scripts/test.mjs` → EXIT=0。Rust：通過（cargo test --workspace --exclude baton-desktop，全部 test result: ok）；前端：29 個測試檔、614 項測試全部通過。
- 真實瀏覽器的測試以 `--nocapture` 重跑，確認是實際執行而不是略過：`web_verification_real_browser` 1 passed、`browser_driver` 2 passed（其中只有「沒有 Playwright」那項因為設定了 BATON_PLAYWRIGHT_DIR 而依設計略過）、`web_verification_adapter` 6 passed。
- `node scripts/build.mjs` → EXIT=0，產生 target/release/baton-desktop.exe（另有 baton-api shared_settings.rs:20 的 unused import `StoreError` 警告，那支檔案不在本票 diff 內，警告在本票之前就存在）。
- 跑完後 `git status --short` 是乾淨的。
FIXES_NEEDED:
1. 畫面顯示秘密時不保存證據檔案：verification_adapter.rs 在瀏覽器步驟取得畫面（`BrowserOutcome::Done`，以及帶畫面的 `ActionFailed`）後，檢查 page.url、page.title、page.text 是否含有任何秘密的值（例如用 `policy.redact` 比對）。有的話：
   (a) 這一步的截圖記為 `withheld: "secret_detected"`，不寫檔；
   (b) 把這個瀏覽器 Session 標成已沾染秘密（可沿用 `secret_entered` 或另加 `secret_shown`），讓這個 Session 的 trace 與錄影也不保存（原因用 `secret_detected`，或另取一個原因碼）。
   `withhold_reason` 的位元組檢查可以保留，當作第二道防線。
2. 修正 fake 與正式實作的分歧：`FakeBrowserDriver` 的截圖改成不含畫面明文的不透明位元組（例如固定的 PNG 檔頭加上雜湊），trace 與錄影也不要依賴明文。這樣 `secrets_reach_only_the_runner_and_never_the_prompt_or_the_evidence` 才是在驗證第 1 點的新邏輯。另外在同一個情境測試補一個「沒有輸入秘密、但畫面顯示秘密」的 Session（例如顧客旅程的頁面顯示 API 權杖），斷言它的截圖、trace、錄影都不保存。
3. （建議）在 web_verification_real_browser.rs 加一個真實頁面顯示秘密的步驟，斷言正式 Runner 的截圖是 withheld。
4. 在 ai-decisions.md 追加一條決策：畫面顯示秘密時證據檔案的處理方式，並標記重要度（建議「高」，屬於秘密政策）。必要時同步更新 docs/development.md 的〈證據檔案〉段落。
```
