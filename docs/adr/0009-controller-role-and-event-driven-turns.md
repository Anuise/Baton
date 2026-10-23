# ADR-0009：Controller 協調角色、事件觸發與依序處理

- 來源識別：DR-20261022-01
- 狀態：已確認（使用者核准，2026-10-22）；**部分被 [ADR-0014](0014-native-cli-controller.md) 取代**（A1、B1）；實作未查驗，CLI 與 Windows 整合為 Validation Required
- 後續：A1（事件驅動喚醒）與 B1（對話依序處理）由 [ADR-0014](0014-native-cli-controller.md) 取代（ADR-0014 仍為 AI 暫定，待使用者確認），並已於 desktop-controller-cli 票 08 移除：對話頁、事件合併視窗、每輪一次 `claude -p` 的 Controller 輪次與 Controller 自動交接。舊的對話資料保留在資料庫，不遷移、不再寫入。本 ADR 其餘部分（Controller 的協調角色、透過工具提交委託、事件去重與 command_id）仍有效。
- 來源：[issue #3](https://github.com/Anuise/issue-orchestrator/issues/3) §17 DR-20261022-01；§2.4、§15.11
- Spec 對應：[spec.md](../../.scratch/platform-rebuild-v1/spec.md) 的 D-06（Controller 執行方式）、D-31（事件合併視窗）、User Stories 18–20、24

## 背景

平台以 Claude Code 作為 Controller 協調多帳號 Worker，工作不限於程式，也包含研究、文件、審查與合併。需要決定 Controller 負責什麼、何時被喚醒，以及使用者訊息與工作事件如何排序，避免 Controller 變成大量產出的瓶頸或不斷空轉耗用額度。

## 決策

採 A1＋B1：

- Controller 本身是 Claude Code，負責對話、必要資料閱讀、目標釐清、拆工、協調、成果評估與下一步判斷，可做簡短查詢與整理；大量研究、文件、實作、詳細審查、測試與合併交給 Worker。Controller 的評估不取代必要的獨立審查。
- Controller 透過工具介面提交委託，由 Runtime 驗證、選帳號，經 Adapter 啟動 Worker；不直接建立未受 Runtime 管理的 Worker。帳號不固定綁定工作角色。
- **A1（事件驅動）**：Runtime 保存使用者訊息、Worker 成果與阻礙等事件，需要 Controller 判斷時才觸發下一輪，不反覆呼叫模型巡查。長期對話持續存在，不要求同一程序永久存活。
- **B1（依序處理）**：同一 Controller 對話依序處理，不並行驅動多輪。忙碌時新訊息先保存並顯示「已收到／待處理」，本輪結束後先處理使用者訊息，再處理一般工作事件；新訊息不自動中斷已開始的操作。明確的暫停與取消由 Runtime 直接處理，不排在對話佇列之後。
- 事件以穩定識別去重；派工與外部操作沿用 command_id、Outbox 與未知結果查證，不承諾副作用恰好一次。事件不解除暫停、完全退出、授權、安全或恢復阻擋。

Spec 補充（AI 暫定，待使用者確認）：D-06 暫定每輪 Controller 判斷為一次 `claude -p` 呼叫，以原生 Session resume 接續；D-31 暫定事件合併視窗為 5 秒。

## 替代方案

- Controller 承擔大量產出：增加協調占用。
- 持續以模型巡查：增加模型用量。
- 訊息立即中斷進行中的操作：增加中斷與恢復成本。

## 後果

- 使用者訊息可能要等本輪結束才被處理，平台不提供回應時間保證。
- 需要額外的派工、持久化與排序機制。
- Controller 無可用帳號時保存事件並顯示等待；Runtime 在沒有可用模型時仍可保存資料、顯示裁定與監控。
