```
VERDICT: PASS
CRITERIA:
- [PASS] 只清理平台登記的 worktree，原始 checkout 與其他目錄不受影響 — 每次刪除前都會先檢查目標：只有 repo 登記、簽出平台分支 `baton/<Task>`、而且沒有未保存檔案的 worktree 才會刪；主 checkout、一般目錄、簽出其他分支的目錄都不刪（crates/baton-domain/src/cleanup.rs:check_safety）。觸發條件是 Kind=Worktree、Task completed、最新一次整合是 integrated、這個 Task 的 Run 全部結束（cleanup.rs:should_request）。正式版的 git 實作只認主 checkout 以外的登記 worktree，其他一律拒絕且不刪任何東西（crates/baton-ports/src/git.rs:git_remove_worktree / registered_worktree，用 .skip(1) 排除主 checkout）。測試證據：worktree_cleanup.rs:142 a_completed_and_integrated_task_has_only_its_registered_worktree_removed、:313 a_directory_that_is_no_longer_the_registered_worktree_is_never_deleted，以及真實 git 測試 git::tests::main_checkout_and_plain_directory_are_foreign_and_never_removed，全部 ok。
- [PASS] 模擬檔案占用造成移除失敗：保留待清理狀態並重試，Task 仍顯示已完成 — worktree_cleanup.rs:207 測試確認：失敗後狀態維持 pending、失敗原因含「被其他程式占用」、next_attempt_at 是 +60s，重試後變成 cleaned，Task 一直是 completed。:249 測試確認重試 5 次（1/2/4/8/10 分鐘）後變成 failed，worktree 保留、Task 仍是 completed。另有真實 Windows 占用測試 git::tests::removal_fails_without_changes_while_a_file_is_held_open（share_mode(0)），結果 ok。UI 上 Task 狀態和清理狀態分開顯示（web/src/TaskBoard.tsx CleanupStatusLine，以及 TaskBoard.test.tsx）。
- [PASS] 清理中崩潰後重建 Runtime，核對後完成清理且不誤刪 — 每次嘗試開始前會先把狀態 `removing` 寫進資料庫，再到交易外移除（crates/baton-runtime/src/supervisor.rs:clean）。worktree_cleanup.rs:340 測試確認：改名到 trash 後 future 被丟棄（模擬崩潰）→ restart → 重新檢查（Missing）→ 狀態變成 cleaned，attempts=1，其他 Task 的 worktree、README、分支都還在。真實 git 的對應測試 removal_finishes_after_a_crash_between_rename_and_cleanup 結果 ok。
- [PASS] 不刪除 Git 分支、歷史或正式 Artifact — 移除只做 rename、`git worktree prune`、刪除 trash，沒有 `branch -D`，也沒有 `worktree remove --force`（git.rs:git_remove_worktree）。worktree_cleanup.rs:189-202 確認：分支 tip 還在、main 上的整合 commit 還在、Artifact 仍可預覽。git::tests::removing_a_clean_worktree_keeps_its_branch_and_everything_else 確認分支 tip 沒變，且可以重建同一個 worktree。
- [PASS] 以 Runtime harness（spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票的 AI 決策已追加至 ai-decisions.md — crates/baton-harness/tests/worktree_cleanup.rs 的 8 個情境（真實 HTTP API 加上 fake WorkspaceFs）全部 ok，涵蓋：只刪登記的 worktree、檔案占用後重試、重試用完、未保存變更擋下刪除、歸屬不明擋下刪除、崩潰後接續、失敗／取消／整合衝突不觸發、完全退出不觸發。AI-246～AI-252 已加入 ai-decisions.md，每條都標了重要度，編號接在 AI-245 之後、沒有重複。
TESTS:
- `node scripts/test.mjs`（結束碼 0）：Rust 全部 test result 加總 1082 passed / 0 failed（包含 baton_domain 424、worktree_cleanup 8、baton-ports 的 git 清理測試 7 個）；前端 vitest 18 個檔案、397 passed。
- `node scripts/build.mjs`（結束碼 0）：baton-runtime release 版和 baton-desktop.exe（tauri --no-bundle）都建置成功。唯一的 warning 是 crates/baton-api/src/shared_settings.rs:20 的 unused import `StoreError`，這個檔案本票沒有改到，問題在 rebuild/v1 就已存在。
- 驗證前後 `git status` 都是乾淨的，沒有改動任何追蹤中的檔案。
FIXES_NEEDED: none
```

**整合分支前移：** 驗證開始時 `rebuild/v1`（014090d）是 HEAD 的祖先。驗證途中它前進到 89d31ef（chore(scratch): 記錄 R077，準備 R090，只有一個 scratch commit），所以現在「rebuild/v1 是 HEAD 的祖先」不成立。依指示不因此判 FAIL，整合交由主控處理。

**不影響判定、但值得留意的幾點：**
1. 被 `.gitignore` 忽略的檔案不會擋下刪除，會跟 worktree 一起被刪掉（AI-247，重要度高，尚未審閱）。這是對「歸屬不明檔案阻擋刪除」的一種解讀，有記錄在決策裡，但建議由人審閱。
2. 檢查和移除前都會先跑 `git worktree prune`。這會順手清掉 repo 裡其他目錄已經不存在的 worktree 登記，影響很小。
3. 「暫停」和「交接」沒有專門的情境測試。這兩種情況不會讓 Task 變成 completed，所以結構上不會觸發清理；失敗、取消和完全退出則都有測試。
