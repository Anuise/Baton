**VERDICT: PASS**

四條功能驗收條件都通過，建置與全部測試也都通過。整合分支在驗證途中前進了一個 commit，前置條件因此不成立，但依指示不影響判定。

**前置條件：** `git merge-base --is-ancestor rebuild/v1 HEAD` 不成立。`rebuild/v1` 前進到 `da904ed`（`chore(scratch): 認領票 30（R108-30-impl）`），只改了 `.scratch/` 底下 3 個檔案，沒有程式碼。實作者合併時的整合點 `06b1659` 仍是 HEAD 的祖先（`--is-ancestor 06b1659 HEAD` 成立）。

```
VERDICT: PASS
CRITERIA:
- [PASS] 關閉視窗後殺掉 Runtime，監護自行重啟並經過完整恢復核對 — 情境測試 crates/baton-harness/tests/guardian.rs:141-186：先寫監護日誌再啟動（:156）；匯入後恢復狀態是 attempting 1/1，處置前不碰任何程序（:159-165）；supervise 後受控停止舊程序、以新 Run 恢復、只扣一次（:170-181）。實際程序也驗證過：release 版 baton-runtime.exe 加 baton-guardian.exe，用 Stop-Process 殺掉 Runtime（pid 37264），約 5 秒內監護拉起新的 Runtime（pid 36208）；journal.jsonl 有 1 筆 seq=1 的 attempt；/api/v1/recovery 顯示 status=observing、attempts_used=1、attempts 只有 1 筆。
- [PASS] 監護日誌與資料庫銜接不重複扣次；啟動早期失敗也計次 — guardian.rs:191-226：restart 之後 attempts_used 仍是 1；再被殺掉時算成第 2 次，並等 30 秒退避（29,999 ms 時不啟動）。guardian.rs:229-273：三次早期失敗依序 seq 1/2/3，退避 30 秒與 2 分鐘，之後 Finished(Exhausted)；開 App 時匯入 3 筆，結果是 exhausted、不派工。去重實作在 crates/baton-store/src/recovery.rs import_guardian_journal（依 attempt_id 查 guardian_journal_import），並有 store 與 domain 的單元測試。
- [PASS] 完全退出與監護啟動競態時不拉起 Runtime — guardian.rs:278-296：完全退出後 Finished(Suppressed(FullExit))，日誌是空的。:299-335：停止意圖只寫進資料庫時，監護拉起的 Runtime 以 SuppressedByStopIntent 結束、不提供服務，並把停止意圖補進生命週期檔，之後監護結束。:338-371：寫下嘗試後才出現維護抑制時，寫入撤回紀錄、不啟動、不計次。:374-393：退避中出現解除安裝抑制，監護結束。實際程序也驗證過：POST /lifecycle/exit 回 202，12 秒後沒有任何 baton-runtime 程序，guardian.log 最後一行是「監護結束，不再重啟 Runtime：Suppressed(FullExit)」，日誌沒有新增嘗試。
- [PASS] 殺掉監護：工作繼續、UI 顯示降級，重開 App 只補監護 — guardian.rs:398-440：Run 和停止次數都不變，health.guardian 是 unavailable 並附「工作繼續，自動重啟保護不可用」；開 App 後 boot_count 不變、恢復事件不重設。crates/baton-client/tests/guardian_client.rs 涵蓋 ensure_guardian 只補監護；web/src/StatusBar.tsx 加了降級提示，有 StatusBar.test.tsx。實際程序也驗證過：殺掉監護後 health 立刻變成 {"state":"unavailable","message":"工作繼續，自動重啟保護不可用"}，Runtime 的 pid 不變；重新啟動監護後變回 active、boot_count 仍是 2；第二個監護因具名 mutex 立即結束。
- [PASS] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；AI 決策已追加至 ai-decisions.md — crates/baton-harness/tests/guardian.rs 有 9 個情境測試，全部 ok，使用真實的 Runtime、SQLite 與 HTTP API，時間由 fake Clock 控制，測試名稱用領域詞彙。ai-decisions.md 新增 AI-312～AI-318，重要度依序為高、高、高、中、中、低、低；diff 沒有任何刪除行，確認只有追加。
TESTS: `pnpm build` → 成功（release 版 baton-runtime、baton-guardian、baton-desktop 都建置完成；只有一個 baton-api/src/shared_settings.rs:20 的 unused import 警告，本票沒有改過這個檔案，原本就存在）。`pnpm test` → 結束碼 0：Rust 1321 passed / 0 failed（有幾個 live_ 與契約測試是 ignored），前端 458 passed（458）。其中 baton-harness tests/guardian.rs 9/9、baton-guardian lib 9/9、guardian_client 5/5、domain guardian::tests 與 store recovery 匯入測試全部通過。另外以 release 版執行檔手動做了真實程序的端到端驗證（見上面各條），暫存資料目錄已刪除。git status 是 clean。
FIXES_NEEDED: none
```

以下幾點不影響判定，記下來供後續參考：

1. **撤回的嘗試可能被多算一次。** 這個空窗很窄：監護寫下 Attempt 之後、寫入 Withdrawn 之前，如果使用者剛好開 App 拉起 Runtime，而 Runtime 先匯入了日誌，這筆撤回的嘗試就會被算成一次失敗。結果是在偏保守的方向多算 1 次，不會少算。相關程式在 `crates/baton-guardian/src/lib.rs:152-165` 和 `crates/baton-store/src/recovery.rs` 的 `import_guardian_journal`。
2. **`OpenProcess` 失敗會被當成程序已結束。** 位置在 `crates/baton-guardian/src/process.rs:265-267`，例如權限不足時也會判定為結束，跟「只認確認過的終止」原則有一點落差。同一使用者的程序實際上很難遇到。
3. **遇到停止意圖時，事前已經做了一點工作。** 監護拉起的 Runtime 在發現停止意圖之前，已經綁定埠並輪替了桌面憑證（`crates/baton-runtime/src/lib.rs` 的 `start_with`）。它不會提供服務，影響很小。
