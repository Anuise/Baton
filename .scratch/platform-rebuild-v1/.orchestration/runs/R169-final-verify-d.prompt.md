你是 platform-rebuild-v1 最後收尾修正的驗證者（run R169-final-verify-d），以唯讀方式驗證分支 `rebuild/final-fix-d`。驗證格式與規則完全依照 `.scratch/platform-rebuild-v1/.orchestration/final-verify.template.md`。

要核對的項目：`/c/Users/User/Documents/au-program/issue-orchestrator/.scratch/platform-rebuild-v1/.orchestration/runs/R168-fix-d.prompt.md` 列出的三項；修正回報在 `/c/Users/User/Documents/au-program/issue-orchestrator/.scratch/platform-rebuild-v1/.orchestration/runs/R168-fix-d.result.md`。

重點：
- Web 確認授權確實帶 expected_revision，並行編輯情境測試能抓到缺陷（必要時暫時改壞再還原）。
- clippy 錯誤是靠修程式碼清掉，沒有大範圍 `#[allow]`；`pnpm test` 確實會跑 clippy。
- 用專用 TEMP 跑完整 `pnpm test`（設定 `BATON_PLAYWRIGHT_DIR=%LOCALAPPDATA%\Temp\baton-pw`），確認沒有殘留。
