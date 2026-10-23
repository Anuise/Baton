RESULT: DONE
COMMITS: 5d1402b..6df628d
TESTS:
- `pnpm test` 沒有整支一次跑完，因為一次跑會超過前景 600 秒的上限。我把它包含的四段分開，都在前景跑：
  - `cargo clippy --workspace --all-targets -- -D warnings`：通過。
  - `cargo test --workspace --exclude baton-desktop`：分四段跑，全部通過。第一段跑完 89 個測試程式、沒有失敗，跑到 `tasks` 時撞到逾時被終止（結束碼 143）。剩下的部分另跑：baton-harness 從 `tasks` 起的 16 個測試程式、baton-mcp-shim／baton-ports／baton-runtime／baton-store、harness 的 lib 與 doc 測試，都通過。
  - `pnpm --filter web test`：30 個檔、678 個測試通過。
  - `pnpm test:e2e`（`BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw`）：通過，4 個旅程。
- `pnpm --filter web typecheck`：通過。
- `cargo check -p baton-desktop`：通過。
- 加壓：
  - Rust：改動的 12 個 harness 測試程式，每輪各 5 份同時跑、每份 16 執行緒，共 2 輪，120 次全部通過。改之前用同樣方式（6 份同時跑）重現到 `worktree_cleanup` 6 次失敗 2 次，失敗在崩潰時還沒改名到暫放目錄。
  - 前端：3 套同時跑 ×4 輪，12 次全部通過。
- `cli/v1` 沒有新 commit，不必 merge；工作樹乾淨。

FIXED:
- R028 第 1 項／review-notes #1、#2、#5 — 舊寫法用 300 或 500ms 的真實時間逾時丟棄 supervise，高負載下會在走到停住點之前就丟棄。新增 `Harness::supervise_until(reached)`，並在 fakes 加上「已走到停住點」的通知：`wait_for_held_removal`、`wait_for_artifact_hang`、`CommandRunner::wait_for_held`、`wait_for_http_hang`。12 個測試檔改成等通知才丟棄這次處理，斷言都沒改，`docs/development.md` 同步更新 — 上述加壓先重現失敗、改後全過（AI-50）。
- R028 第 2 項 — 時間線的 Controller CLI 事件改為顯示帳號名稱（用帳號清單對照 `profile_id`，對照不到就顯示 id）、Controller CLI id 與結束代碼 — `TimelinePage.test.tsx` 新增 2 個測試，修之前會失敗（AI-51）。
- R028 第 3 項／review-notes #8 — 輪詢測試不再用 `shouldAdvanceTime`，改用完全的假計時器加 `advanceTimersByTimeAsync`。到達順序不固定的請求，改成等明確的完成狀態再斷言。`test-setup.ts` 把 findBy／waitFor 的等待上限統一放寬到 3 秒 — 用延遲注入重現（ControllerPage 6 個、ExplorationPage 2 個測試失敗），修後通過，加壓也通過（AI-52）。
- R028 第 4 項 — 「無法取得帳號清單」的提示移到 Controller CLI 開著與未開啟共用的位置 — 新增「開著時帳號清單讀取失敗」的元件測試，修之前會失敗。
- R028 第 5 項 — GLOSSARY 更新四個條目：MCP shim 補上 `controller-host`、`wait` 子命令與 `BATON_MCP_SHIM`；Controller 憑證在結束或 `rebound_at` 時失效；等待原因補上 `controller_account`；Account Profile 補上 `config_dir_origin`，並註明不刪設定目錄 — 純文件修改，沒有測試。
- R028 第 6 項 — `open_controller_cli` 改為先確認這次的 Run 已是 `running`，再用它的 `command_id` 找開窗紀錄取憑證 — 新增 should_panic 測試，修之前會失敗。

NOT_FIXED:
- 分給 final-c 的項目全部修了，沒有決定不修的。
- 新發現一項不在分派範圍內的問題，沒有處理：更重的加壓（3 套完整前端測試同時跑）下，`VerificationPage.test.tsx` 失敗過 3 次。2 次是 `user.type` 逐字輸入一長串 JSON 超過每個測試 5 秒的上限，1 次是送出時少了 `task_id`。已記為 review-notes 第 11 項。

NOTES:
- 沒有跨範圍修改：除了 `.scratch` 的紀錄（ai-decisions.md、review-notes.md）之外，只動了本範圍的檔案。
- AI 決策我用了 AI-50～52。final-ab 也會從 AI-50 起接續編號，合併時很可能撞號，需要整合時重新編號。
- 請驗證者特別看以下幾處：
  - `run_process_release` 的「慢的停止不擋住其他取消」沒有單一停住點，改成在 supervise 進行中輪詢「快的 Run 已是 cancelled」。如果停止被改成一個接一個執行，這支測試會撞到 20 秒的保險逾時才失敗，錯誤訊息不像原本那樣具體。
  - 改寫的 harness 測試大多來自 platform-rebuild-v1，不是本 feature 的測試。審查報告點名了它們，所以一併改了。
  - review-notes 第 1 項（R005 那次結束碼 101）當時沒有保存輸出，無法確認是哪一支測試。我根據時序推定它和第 2、5 項是同一個成因，所以一起算已解決。
- 前端四項是由一個 subagent 實作的（只改 `web/`）。我審過它的 diff、修正了一處擺錯位置的文件註解，再自己重跑測試與 typecheck 後才 commit。它為了跑測試在 worktree 裝了 node_modules（`pnpm install --frozen-lockfile`），這個目錄不進版控。
exit: 0

