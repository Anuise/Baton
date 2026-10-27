驗證結果：通過。前次 R137 不通過的原因是「畫面顯示秘密時證據檔案仍被保存」，這次已確實修正，建置與全部測試也都通過。

```
VERDICT: PASS
CRITERIA:
- [PASS] 前置：rebuild/v1 是 HEAD 的祖先 — 驗證開始時 `git merge-base --is-ancestor rebuild/v1 HEAD` → ANCESTOR_OK（當時 rebuild/v1 = d986776，已在 merge 2c8fc0e 中）。驗證途中整合分支前進到 82141ce（只多一個 commit：「chore(scratch): 記錄 R136，準備 R140」），所以重跑時前置條件不成立。依指示只標註，不判 FAIL。
- [PASS] 模型限流不被記為產品缺陷 — R137 之後這部分沒有改動（判定邏輯在 verification_profile.rs:1326-1328、:1441-1442；限流後同一次驗證不再呼叫模型，在 verification_adapter.rs 的 judge 流程）。情境測試 `a_rate_limited_model_is_never_recorded_as_a_product_defect` 和 `without_a_usable_account_the_judge_is_unavailable_not_a_failure` 都通過（用 --nocapture 重跑確認）。
- [PASS] 秘密不出現在 Prompt 或證據中 — Prompt：judge 收到的網址、標題、畫面文字都經過 redact（verification_adapter.rs:234-240）。證據檔案：R137 的缺口已修正。
  (1) 判斷改看畫面內容：新增 `shows_secret` 函式，用網址、標題、畫面文字比對遮蔽清單（verification_adapter.rs:97-100）。`Execution::screenshot` 對 `Done` 與帶畫面的 `ActionFailed` 都會執行（:192、:205、:214-221）。即使截圖政策是 off，這個 Session 也會被標成 `secret_shown`。
  (2) 保存檔案時把這個標記傳下去：截圖帶的是這一步的 `shows_secret`（:280-284）；trace 和錄影帶的是整個 Session 的 `secret_shown`（:404-410）。`withhold_reason` 碰到這個標記就回傳 `secret_detected`（verification_profile.rs:1643-1660）。原本比對檔案位元組的檢查保留，當作第二道防線。
  (3) fake 與正式實作的分歧已修正：fake 的截圖改成 PNG 檔頭加 sha256，trace 改成 zip 檔頭加 sha256，不再含畫面明文（fakes.rs:1710-1715、:1777-1778、:1804-1806）。所以現在 `whoami` 和 `browse/sort` 的截圖判為 `secret_detected`，確實是新邏輯造成的（web_verification_adapter.rs:456-475）。
  (4) 補上「沒輸入秘密、但畫面顯示秘密」的 Session：截圖、trace、錄影都判為 `secret_detected`；顯示秘密之前的截圖照常保存；完全乾淨的 Session 三種檔案都照常保存（:499-505）。
  (5) 真實瀏覽器測試新增 `/status` 頁，把密碼顯示在畫面上。斷言正式 Runner 交回的 PNG 是 `secret_detected`、整個 Session 的 trace 也是 `secret_detected`（web_verification_real_browser.rs:54-55、:177-181）。用 --nocapture 重跑確認是實際執行，沒有被略過。
  另外確認：runner.mjs:108 回傳的 innerText 沒有先遮蔽，所以 Rust 端比對得到秘密。
- [PASS] 測試 persona 與實際登入身分分開綁定 — R137 之後沒有改動。`test_personas_and_login_identities_are_bound_separately` 通過。
- [PASS] 以主要 seam 撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加 — crates/baton-harness/tests/web_verification_adapter.rs 的 6 項情境測試，加上 web_verification_real_browser.rs 的 1 項，全部通過。ai-decisions.md 已有 AI-408～AI-415。AI-415（ai-decisions.md:3865，重要度：高）記錄了畫面顯示秘密時證據檔案的處理方式、理由、替代方案和已知限制。docs/development.md:1260〈證據檔案〉段落也同步更新了。
TESTS:
- `BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw node scripts/test.mjs` → EXIT=0。Rust（cargo test --workspace --exclude baton-desktop）108 個 test result 全部 ok，沒有 FAILED；前端 Test Files 29 passed、Tests 614 passed。
- `cargo test -p baton-harness --test web_verification_real_browser --test web_verification_adapter -- --nocapture`（透過 scripts/lib.mjs 的 MSVC wrapper）→ real_browser 1 passed、adapter 6 passed，沒有略過訊息。
- `node scripts/build.mjs` → EXIT=0，產生 target/release/baton-desktop.exe。唯一的警告是 baton-api shared_settings.rs:20 的 unused import `StoreError`；這支檔案不在本票的 diff 內，警告是本票之前就有的。
- 跑完後 `git status --short` 是乾淨的。
FIXES_NEEDED: none
（不影響判定的已知限制，AI-415 已記錄，留給後續追蹤：秘密只出現在 `<input>` 的值、不在 innerText 中時，偵測不到；步驟之間短暫出現的中間畫面也檢查不到。這兩種情況下，截圖或 trace 仍可能拍到秘密。）
```
