```
RESULT: DONE
COMMITS: c9c0a54..12d8940
TESTS:
- 所有測試都在專用 TEMP（%LOCALAPPDATA%\Temp\baton-final-c）執行，並設定 BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw
- pnpm test → exit 0。Rust 1709 通過、0 失敗、9 ignored（多出的 1 個是新加、要以子程序執行的子測試）；前端 30 個檔案、648 個測試通過。連跑兩次，結束後 TEMP 都只剩一個空的 baton-harness\，沒有 .tmp*，也沒有 Baton 資料目錄。
- pnpm --filter web typecheck → exit 0
- cargo check -p baton-desktop（經 with-msvc.ps1）→ exit 0
- 另外跑的：
  - cargo test -p baton-harness 全部通過，結束後 baton-harness\ 是空的
  - harness_cleanup 的新測試在修正前失敗，留下 ["h0JZT76", "h0JZT76.lock"]；修正後通過
  - baton-runtime 每個整合測試修正前逐一跑 3 次、修正後再跑 4 次：修正前 browser_driver 每次留 1～2 個 .tmp*、command_runner 每次留 1 個；修正後都沒有殘留
  - browser_driver 以真實 Playwright 跑 2 次，沒有殘留
- 結束後已刪除專用 TEMP 目錄與暫時的腳本；工作樹乾淨，rebuild/v1（6ca1459）是 HEAD 的祖先

FIXED:
- R158 FIXES_NEEDED 第 1 項（review-notes #28）— 修法：data_dir.rs 把刪不掉的目錄記到全域待刪清單，背景執行緒照舊重試；另外在第一次有目錄進入清單時以 libc::atexit 註冊程序結束時的掛勾，同步重試刪除剩下的目錄，上限 10 秒（baton-harness 新增 libc 依賴）— 測試：harness_cleanup.rs 新增 nothing_is_left_after_the_test_process_exits。它以子程序執行一個持有 harness 到測試結束的測試，子程序的 TEMP/TMP/TMPDIR 指到專用目錄，結束後斷言 baton-harness 之下沒有任何目錄或 .lock。
- R158 FIXES_NEEDED 第 2 項（review-notes #28「檢查其他測試」）— 修法：新增 crates/baton-runtime/tests/support/mod.rs 的 remove_temp_dir，在測試結尾有上限（10 秒）地重試刪除並斷言刪掉。用在 command_runner 的逾時測試、browser_driver 的兩個測試（含略過的路徑）、supervisor_process_tree。根因是產品以 kill_on_drop 終止子程序但不等它結束，或被終止的程序樹還沒放掉 .baton。全 repo 沒有其他 into_path/keep 的用法。同步改寫 AI-483（原 AI-457）中「其他 crate 不在範圍內」的說法，新增 AI-484 — 測試：在專用 TEMP 逐一重跑與完整 pnpm test 後確認沒有殘留（修正前後的數字見 TESTS）
- 合併 rebuild/v1 時的 AI 編號撞號：本分支的 AI-449～AI-457 順延為 AI-475～AI-483（整合分支在途中新增了 AI-474，所以順延了兩次），docs/development.md 的引用同步改為 AI-481。自動合併兩次都吃掉了本分支最後一條的「- 審閱：未審」，已補回。已確認整合分支既有條目沒有任何刪改。

NOT_FIXED:
- R158 FIXES_NEEDED 第 3 項（UI E2E 開票）：依指示由主控處理（整合分支已有 AI-474／票 53）。
- 產品端的 LocalCommandRunner 逾時與 LocalBrowserDriver 開啟失敗時，被終止的子程序沒有被等到結束：這屬於產品程式，本次只允許改測試檔，所以沒改，理由與影響記在 AI-484。

NOTES:
- 跨範圍修改：crates/baton-runtime/tests/ 下的 browser_driver.rs、command_runner.rs、supervisor_process_tree.rs，以及新增的 support/mod.rs。只改測試，依指示允許。
- 給主控（我沒有改這幾處）：
  - 整合分支 AI-474 的理由寫「（AI-455）」，指的是本分支原本的 AI-455，現在是 AI-481；整合分支的 AI-455 是另一條決策。依規定不可修改整合分支既有條目。
  - runs.md 中 R156-fix-c 那一列仍寫 AI-455／AI-457，應為 AI-481／AI-483。原本我改了這一列，但下一次合併時自動合併把列重複了一份，所以整個檔案還原為整合分支的版本（12d8940）。
- 驗證者請特別看：
  - remove_pending_at_exit 在 atexit 中執行，不會 panic：lock 中毒時改用 into_inner，也沒有 unwrap。
  - TEMP 會留下空的 baton-harness 容器目錄，這是刻意的：程序結束時刪它，會和並行程序在其中建立目錄的動作競爭，說明在 AI-483。
- 偶發失敗：修正前逐一重跑 runtime 測試時，single_instance_process 在 11 次中失敗 1 次（沒有留下訊息）。這個測試和本次修改無關，之後在完整 pnpm test 中都沒有再出現；建議另外觀察。
```
