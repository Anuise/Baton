# ADR-0012：以 Runtime harness 與注入式 Port 作為唯一主要測試 seam

- 來源識別：D-04
- 狀態：**AI 暫定，待使用者確認**（spec 的 Claude 決策 D-04；改動成本：高）
- 來源：[spec.md](../../.scratch/platform-rebuild-v1/spec.md)「Claude 決策索引」D-04、「Testing Decisions」；issue #3 的故障恢復要求見 [issue #3](https://github.com/Anuise/issue-orchestrator/issues/3) §12、§15
- 相關：[ADR-0005](0005-runtime-crash-recovery.md)、[ADR-0006](0006-runtime-guardian.md)

## 背景

平台的主要風險集中在故障邊界：spawn 已執行但回執未寫回、交接中途崩潰、授權與准入競態、完全退出與重啟。這些行為橫跨 Domain、儲存、排程、程序管理與 API。若測試貼著內部函式與資料表欄位，重構就會讓大量測試失效；若只做 UI 端到端測試，故障注入與時間控制又難以進行。本 repo 目前沒有可沿用的測試程式碼。

## 決策

以「Runtime API + 注入式 Port」為**唯一的主要測試 seam**：

- **Runtime harness**：在測試中以程序內方式啟動完整 Runtime，使用真實的 Domain 核心、真實的 Store（暫存目錄中的真實資料庫）與真實的 Scheduler／Supervisor／Outbox，只替換 Port：Clock、ProcessControl、AgentAdapter、QuotaObserver、WorkspaceFs、PlatformInfo。測試透過真實 API，以桌面與 Agent 兩組憑證操作。
- 「重啟 Runtime」＝丟棄實例、保留資料與檔案、以同一組 fake 重建。
- 好測試只斷言外部行為（API 回應、可觀察狀態、事件、Port 被呼叫的方式，例如「沒有第二次 spawn」），不斷言資料表欄位或內部呼叫順序。
- 時間一律由 fake Clock 控制，不使用真實 sleep；fake Port 可在任何邊界模擬崩潰（故障注入是一級公民）。
- 補充而非取代：Adapter 契約測試（同一份契約同時跑在 fake 與真實 Claude Code Adapter）、少量 UI 端到端測試、Domain 核心的性質測試、Windows 平台驗收清單。

## 替代方案

- 貼近內部的單元測試為主：重構成本高，且難以驗證跨模組的故障恢復。
- 以 UI 端到端測試為主：慢、不穩，難以注入故障與控制時間。
- 以真實 Claude Code 與真實程序做整合測試：需要帳號與 Windows 環境，無法作為日常回歸。

## 後果

- 所有外部副作用都必須經由 Port 注入；新增副作用時必須先定義 Port 與對應 fake。
- 第一張實作票必須先建出 Runtime harness，其他票才能依賴它。
- fake 與真實 Adapter 的行為差異靠契約測試把關；契約測試的真實版只在已登入測試帳號的 Windows 環境執行。
- 這個決策改動成本高：若日後要改成其他 seam，既有測試的大部分都要重寫。
- 本決策為 AI 暫定。`ai-decisions.md` 的「Spec 決策審閱紀錄」載有使用者於 2026-10-22 對 D-04 的認同，但本 ADR 在使用者於 ADR 層級確認前，仍依票 02 的驗收條件標示為待確認（見 AI-21）。
