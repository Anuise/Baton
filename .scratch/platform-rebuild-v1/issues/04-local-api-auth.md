# 04: 本機 API 身分驗證與網頁存取防護

Status: completed
Blocked by: 01

**What to build:** Runtime API 只綁定 IPv4／IPv6 loopback；所有端點（含 SSE 串流）要求 bearer token 與固定自訂標頭，檢查 Host 與 Origin，拒絕 CORS 預檢（spec D-23）。桌面管理憑證於每次 Runtime 啟動時輪替並寫入 DPAPI 保護檔，App 自動讀取連線，使用者不需複製密鑰（spec D-22）。資料庫或記憶體只存 token 雜湊。為後續 Agent 憑證預留角色欄位（桌面／Agent）。

**Blocked by:** 01 專案骨架與 Runtime harness

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 缺少、錯誤或過期憑證的請求被拒絕，包括串流
- [x] 帶有任意網頁 Origin 或非 loopback Host 的請求被拒絕；不回應 CORS 預檢
- [x] Runtime 重啟後 App 自動以新憑證重連，驗證失敗時顯示原因、不降級為免驗證
- [x] 憑證不出現在 URL、日誌或錯誤訊息中（測試以日誌掃描驗證）
- [x] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
