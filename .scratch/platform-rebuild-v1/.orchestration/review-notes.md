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
