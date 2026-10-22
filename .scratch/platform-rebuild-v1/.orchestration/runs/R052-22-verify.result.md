VERDICT: PASS
CRITERIA:
- [PASS] 前置：`rebuild/v1` 是 HEAD 的祖先。`git merge-base --is-ancestor rebuild/v1 HEAD` 回傳 0。
- [PASS] Agent 憑證無法提交正式裁定。有三層擋住：
  - 回答端點在 `/api/v1/` 之下，guard 對 Agent 憑證回 403 `role_not_allowed`。
  - Domain 在 `decided_by` 不是 desktop 時拒絕（`NotDesktop`）。
  - 資料庫有 CHECK：`decided_by = 'desktop'`。
  - 測試 `crates/baton-harness/tests/decisions.rs:471` 通過，`decision` 表為 0 筆。
- [PASS] 裁定後受影響的 Task 自動恢復排程，無關的 Task 從未被阻擋。
  - `decision.rs` 只恢復仍在 `waiting_decision`、且沒有其他待處理請求擋著的 Task。
  - 在同一個交易裡重新排進 `dispatch_request`，並追加 `decision_wake:` Outbox（`crates/baton-store/src/decisions.rs` 的 `save()`）。
  - Run 結束只會轉換 `Running` 的 Task（`run.rs:468`、`:568`）。
  - 測試 `:252`、`:355`、`:373` 通過：Task B 全程維持 running，只有 1 個 Run。
- [PASS] 逾時不視為同意。期限只用來算 `is_overdue`。
  - 測試 `:541` 推進時間超過期限、重啟、再推進 48 小時後，請求仍是 pending，Task 仍在 `waiting_decision`。
  - 測試 `:575` 推進 72 小時後，權限提示仍回 `deny`／`waiting_decision`。
- [PASS] 權限提示授權內自動允許、範圍外產生裁定請求（以 fake Adapter 驗證）。
  - 授權內：`permission::evaluate` 判定 Allow，並追加 `permission_auto_allowed` 事件。
  - 範圍外：建立 `allow_once`／`deny` 的裁定請求，回 `behavior:"deny"`、`status:"waiting_decision"`。
  - `decisions.rs:594-790` 的 7 個權限情境，以及 shim 測試 `permission_prompt_is_forwarded_and_a_waiting_deny_is_not_a_tool_error`，全部通過。
- [PASS] 情境測試以 Runtime harness（spec D-04）撰寫，AI 決策已追加。
  - `decisions.rs` 共 20 個情境全部通過，走真實 HTTP API 與 fake Port。
  - `ai-decisions.md` 已追加 AI-128 到 AI-136，都有標記重要度。
- [PASS] 票面 What to build：
  - `web/src/DecisionCenter.tsx` 有清單與單筆詳情，顯示問題、原因、選項／取捨、建議、證據、阻擋範圍、逾期標示。
  - Tauri 命令 `decision_list`、`decision_detail`、`decision_answer`、`decisions_pending` 已接上。
  - 狀態列有待裁定數。

TESTS:
- `pnpm test`：結束碼 0。
  - Rust 全部 ok，沒有 FAILED／panicked（baton-domain 251 個、decisions 20 個）。
  - 前端 Vitest 14 個檔案、227 個測試通過。
- `pnpm build`：第一次因為 C 槽沒有空間而失敗（`os error 112`，環境問題，不是程式錯誤）。磁碟騰出約 19G 後重跑，結束碼 0，產出 `target/release/baton-runtime.exe` 與 `target/release/baton-desktop.exe`。

FIXES_NEEDED: none

不影響判定的觀察，建議交給票 14：P3 授權下，寫入 Workspace 內的 `.claude/settings*.json` 會被自動允許，`.git` 則有擋。如果 Adapter 啟動時會載入專案層級的設定，Worker 可能藉此繞過 `permission_prompt`。
