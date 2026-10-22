# 待最終 code-review 修正的已知問題

主控在執行期間收到、尚未派票處理的問題。最終 code-review 的修正 run 必須一併處理本檔所列項目。

## 1. 權限自動允許的私有位址判斷不完整（票 22，permission.rs）
- 來源：2026-10-22 背景安全審查（中）。
- 問題：判斷 URL 主機是否為私有位址時，只拆解「四段十進位」的 IPv4。整數、十六進位、八進位 IPv4（例如 `http://2130706433/`、`0x7f.1`）、IPv6（`[::1]`、`[fd00::]`）、結尾帶點的 `localhost.`、以及解析到私有位址的網域名稱，都會落到最後的 `true` 而被當成公開位址自動允許，形成 SSRF。
- 修法：以 `url::Url` 解析並用 `host()`；`Ipv4`／`Ipv6` 用標準函式排除 loopback、private、link-local、unspecified、CGNAT（100.64/10）、unique-local；網域先去掉結尾點並轉小寫再比對 `localhost`／`.localhost`／`.local`；非標準寫法的 IPv4 一律回傳無法歸類；網域名稱不自動允許，除非在明確的白名單內。補單元測試涵蓋上述每一種繞過。

## 2. 偶發失敗的測試
- `crates/baton-client/tests/artifacts_client.rs::app_download_of_a_missing_version_or_a_tampered_file_is_rejected_with_the_runtime_code`：R043 在全套測試中於派工時失敗一次，單獨重跑皆通過，根因未查。
- `web/src/App.test.tsx`「選到 Workspace 後可在授權設定與工作面板之間切換」：R056 冷啟動時在預設 1 秒等待內找不到按鈕，重跑皆通過。
- `crates/baton-runtime/tests/process_tree.rs::duplicate_command_id_is_rejected`：R045 在完整執行時失敗一次（程序數比對），單獨重跑皆通過。
- 要求：查出根因並修正（例如等待條件或共用狀態），不可只拉長逾時或標記略過。

## 3. 驗證命令的程序樹與路徑檢查（票 13）
- 驗證命令沒有放進 Job Object，逾時後由它再開的子孫程序可能殘留；檔案檢查不處理指向工作目錄外的符號連結或 junction（docs/development.md「已知限制」）。最終修正時評估是否沿用票 10 的 Job Object 與路徑正規化。

## 4. 完全退出的兩個邊界（票 25）
- `request_exit`（API）與 Supervisor 的 `mirror_lifecycle_file` 共用同一個 `baton-lifecycle…tmp` 暫存檔名，同時寫入時一方 rename 會失敗；改為各自唯一的暫存檔名。
- `run_full_exit` 裡的 `process_of`（`ProcessControl.find`／`ensure_managed`）不受 40 秒期限約束，find 卡住時退出可能超時；納入同一個期限。

## 5. 備份還原的兩個小問題（票 40）
- `restore_backup` 直接把 manifest 的 `storage_path` 接到目標路徑，沒有防 `..` 路徑穿越；改為拒絕絕對路徑與任何 `..` 元件。
- `baton-runtime restore` 失敗時的錯誤訊息帶著「Runtime 啟動失敗：」前綴，改用還原專用的訊息。

## 6. Adapter 的兩個觀察（票 14）
- `spawn_spec` 以 `is_file()` 判斷快照檔是否存在，`command_hash` 因而依賴檔案系統狀態；Run 啟動後快照被刪，重建時對回執未寫回的 Run 算出的雜湊會不同。改為只依 Run 綁定的快照版本計算。
- CLI 版本只在核對 Profile 時記錄；降版要到下次核對才會被擋。評估是否在每次啟動前比對。

## 7. 重犯回報靜默丟棄 workspace_id（票 38）
- `POST /api/v1/knowledge/{id}/recurrences` 只帶 `workspace_id`、不帶 `task_id` 時，`crates/baton-api/src/guards.rs` 的 `report` 會靜默丟棄 `workspace_id`。改為保存或明確回 400。

## 8. 訊息字串內多餘空白（票 35）
- `crates/baton-api/src/agent.rs:436` 的 `requires_decision` 回應訊息中間夾了約 17 個空白，改用 `\` 續行或刪除。

## 9. 票 24 的小問題
- `crates/baton-api/src/shared_settings.rs:20` 未使用的 `StoreError` import（編譯警告）。
- `crates/baton-domain/src/authorization.rs:405` 說明文字與 `crates/baton-store/src/runs.rs` 的 `RUN_COLUMNS_QUALIFIED` 有多餘空白（同第 8 項的折行問題）。

## 10. 外部操作查證的兩個設計取捨（票 26）
- 已成功、在 `verifying` 的 Task 若留下未回報的外部操作，查證後回到 `ready` 重新派工，整件工作會重做一次；評估是否改為只重新驗證。
- 已因其他原因等待裁定的 Task 不會被加入「結果未知」請求的阻擋範圍，另一個裁定回答後會先恢復派出，要到新 Run 登記同一個鍵才被擋下（安全但多跑一次）。

## 11. 原生 Session 綁定的邊界（票 15）
- `bind_native` 遇到 `InUse{driven:false}`（另一個暫停中的 Session 綁著同一原生 ID）時不要求取消，但 `run.native_session_id` 仍會被設定；之後該暫停 Session 被另一 Run 以 `--resume` 接續時，可能兩個程序同時驅動同一原生 Session。改為 `driven:false` 時也拒絕綁定（或取消）。

## 12. 票 36 的小問題
- `crates/baton-store/src/scheduling.rs:286-289` 的 `backoff_budget` 插在 `failed_check` 的 doc 註解與函式之間，註解錯掛。
- 驗證不完整時立即重驗，兩次重驗可能數秒內用完（AI-274 的取捨）；評估是否加入短暫延遲。

## 13. 崩潰判定的邊界（票 31，AI-280）
- 正式執行檔在 Ctrl-C 或 `wait()` 結束時不呼叫 `shutdown()`，主控台 Ctrl-C、程序被終止、重開機後自啟動都會被判為崩潰並觸發恢復；確認票 33 是否處理跨登入情境，並評估 Ctrl-C 是否應走正常結束。

## 14. 交接時程序樹確認（票 28，AI-283）
- Worker 自行結束時收到 `Exited` 就轉移，未先用 ProcessControl 確認整個程序樹結束；程序樹殘留子程序時可能短暫出現第二個 writer。轉移前先 `inspect`。

## 15. 監護程序的邊界（票 32）
- 監護寫下 Attempt 後、寫入 Withdrawn 前，若 App 拉起的 Runtime 先匯入日誌，撤回的嘗試會被多算一次（`crates/baton-guardian/src/lib.rs:152-165`、`import_guardian_journal`）。
- `crates/baton-guardian/src/process.rs:265-267` 的 `OpenProcess` 失敗（例如權限不足）被當成程序已結束，與「只認確認過的終止」不一致。

## 16. Controller 工具與 spec D-05 的落差（票 17，AI-327）
- Controller 憑證目前只能呼叫 6 個 Controller 工具與 `permission_prompt`；spec D-05 列為共同工具的 `search_knowledge`、`send_message`、`read_inbox` 對 Controller 不可用，`accept_artifact` 與 Controller 版 `record_lesson_candidate` 尚未實作。確認後續票是否涵蓋，否則補上或在 spec 註記。

## 17. 時間線查詢效能與歸屬（票 41）
- `read_timeline` 每次讀全庫的 TIMELINE_AGGREGATES 事件與全部 task，在 Rust 端過濾；事件量大時會變慢，改為 SQL 端依 Workspace／Task 篩選。
- 擋多個 Task 的裁定請求只歸屬到發起的 Task，其他被擋的 Task 時間線看不到。

## 18. Controller 交接（票 30）
- `stop_controllers_for_handoff`（`supervisor.rs`）找承接帳號時傳入 `active_runs: &[]`，沒考慮各帳號目前並行量；承接帳號滿載時可能停掉舊輪後才發現放不下。
- 補情境測試：唯一帳號 draining 且 Controller 有一輪在執行時，等滿 2 分鐘不停止、留在原帳號（M7）。

## 19. 登入自啟動的舊值（票 33）
- 登錄已有名為 `Baton` 但命令列不同的值（例如舊安裝位置），偏好為關閉時 `sync_login_startup` 視為「未登記」而不移除；改為依值名稱判定歸屬並移除。

## 20. Stage 預算用盡後可被 Controller 直接解除（票 43）
- Stage 失敗次數用盡預算而被阻擋後，Controller 附上有效證據回報 passed 即可通過並解除阻擋；spec 把「預算用盡後的下一步」列為必須送裁定。改為預算用盡後只能經使用者裁定解除。

## 21. 驗證證據的歸屬與排程（票 45）
- Stage 查驗 `verification` 證據時（`verification_evidence_status`）不比對 Bundle 的 `task_id` 是否為 Workflow 綁定的 Task；`task_accepted` 證據同樣沒有比對。改為只接受綁定 Task 的證據。
- 驗證要求與整合佇列共用 `verify_lock` 依序執行，長時間驗證會延後整合。
