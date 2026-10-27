# ADR-0004：第一版桌面入口與 Runtime 存取權限

- 來源識別：DR-20261021-04
- 狀態：已確認（使用者核准，2026-10-21）；憑證交付、失效與重連及 Windows 防護為 Validation Required，尚未驗證
- 來源：[issue #3](https://github.com/Anuise/issue-orchestrator/issues/3) §17 DR-20261021-04；§2.3、§6、§11.2、§12.4、§13、§15.6
- Spec 對應：[spec.md](../../.scratch/platform-rebuild-v1/spec.md) 的 D-22（憑證格式與交付）、D-23（網頁存取防護）、User Stories 57、86–89

## 背景

平台控制入口若可被遠端或一般網頁存取，風險與測試成本都高；若所有呼叫共用一個管理密鑰，一個失控的 Agent 就能批准自己的請求。第一版需要簡化交付範圍，同時保留人工與 Agent 的權責差異。

## 決策

採 A＋A2＋U1：

- 使用者只從同機的 Tauri 桌面 App 操作，React Web UI 內嵌於 App；Runtime API 只監聽本機 loopback，不提供遠端監聽設定或一般瀏覽器管理入口。
- 桌面管理連線與各 Agent 使用不同憑證。Agent 憑證關聯 Workspace、Run、角色與有效世代，只取得委託所需能力。Runtime 每次請求核對呼叫者、資源歸屬與操作授權，不信任請求自填的 Workspace／Run 識別。
- Agent 可以提出 Decision Request 並在委託範圍內工作；不能以 Agent 憑證提交人工正式批准、擴大 Workspace 授權或取得桌面管理能力。
- App 自動完成連線驗證與安全重連，不要求使用者複製密鑰；驗證失敗時顯示原因，不降級為免驗證。

Spec 補充（AI 暫定，待使用者確認）：D-22 暫定為 256-bit 隨機 bearer token（資料庫只存雜湊）、桌面憑證以 DPAPI 保護檔交付、Agent 憑證只走環境變數；D-23 暫定為必帶自訂標頭、Host／Origin 白名單、拒絕所有 CORS 預檢。這些屬於 issue 中標為 Validation Required 的細節。

## 替代方案

- 第一版即支援遠端控制：增加身分、網路與遠端批准契約。
- 所有呼叫共用管理密鑰：擴大 Agent 權限的影響範圍。
- 同時提供一般瀏覽器入口：增加登入、連線與安全測試成本。

## 後果

- 離開那台電腦就不能查看或裁定；第一版沒有一般瀏覽器入口，也不支援多人或遠端管理。
- 分開憑證不是 OS 安全隔離，不防止擁有相同 Windows 使用者權限的惡意程式竊取資料或憑證。
- 此限制只針對平台控制入口，不禁止已授權的 Agent 存取外部服務。
- 憑證不得出現在 URL、Prompt、日誌、transcript、Artifact 與一般備份中。
