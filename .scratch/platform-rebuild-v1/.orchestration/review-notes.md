# 待最終 code-review 修正的已知問題

主控在執行期間收到、尚未派票處理的問題。最終 code-review 的修正 run 必須一併處理本檔所列項目。

## 1. 權限自動允許的私有位址判斷不完整（票 22，permission.rs）
- 來源：2026-10-22 背景安全審查（中）。
- 問題：判斷 URL 主機是否為私有位址時，只拆解「四段十進位」的 IPv4。整數、十六進位、八進位 IPv4（例如 `http://2130706433/`、`0x7f.1`）、IPv6（`[::1]`、`[fd00::]`）、結尾帶點的 `localhost.`、以及解析到私有位址的網域名稱，都會落到最後的 `true` 而被當成公開位址自動允許，形成 SSRF。
- 修法：以 `url::Url` 解析並用 `host()`；`Ipv4`／`Ipv6` 用標準函式排除 loopback、private、link-local、unspecified、CGNAT（100.64/10）、unique-local；網域先去掉結尾點並轉小寫再比對 `localhost`／`.localhost`／`.local`；非標準寫法的 IPv4 一律回傳無法歸類；網域名稱不自動允許，除非在明確的白名單內。補單元測試涵蓋上述每一種繞過。

## 2. 偶發失敗的測試
- `crates/baton-client/tests/artifacts_client.rs::app_download_of_a_missing_version_or_a_tampered_file_is_rejected_with_the_runtime_code`：R043 在全套測試中於派工時失敗一次，單獨重跑皆通過，根因未查。
- `web/src/App.test.tsx`「選到 Workspace 後可在授權設定與工作面板之間切換」：R056 冷啟動時在預設 1 秒等待內找不到按鈕，重跑皆通過。
- 要求：查出根因並修正（例如等待條件或共用狀態），不可只拉長逾時或標記略過。

## 3. 驗證命令的程序樹與路徑檢查（票 13）
- 驗證命令沒有放進 Job Object，逾時後由它再開的子孫程序可能殘留；檔案檢查不處理指向工作目錄外的符號連結或 junction（docs/development.md「已知限制」）。最終修正時評估是否沿用票 10 的 Job Object 與路徑正規化。

## 4. 完全退出的兩個邊界（票 25）
- `request_exit`（API）與 Supervisor 的 `mirror_lifecycle_file` 共用同一個 `baton-lifecycle…tmp` 暫存檔名，同時寫入時一方 rename 會失敗；改為各自唯一的暫存檔名。
- `run_full_exit` 裡的 `process_of`（`ProcessControl.find`／`ensure_managed`）不受 40 秒期限約束，find 卡住時退出可能超時；納入同一個期限。
