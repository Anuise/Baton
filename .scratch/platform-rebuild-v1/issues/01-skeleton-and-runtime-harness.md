# 01: 專案骨架與 Runtime harness

Status: completed
Blocked by: None

**What to build:** 建立第一版的可執行骨架並打通最細的一條路徑：以 git tag 封存現有 skill 版本；建立 Rust workspace（Domain 核心、Store、Runtime、API）、Tauri 桌面殼與 React + TypeScript + Vite Web UI；Runtime 提供一個健康狀態端點，桌面 App 啟動時啟動或連回 Runtime 並在頂部狀態列顯示「Runtime 運作中」。同時建立 spec D-04 的 Runtime harness：在測試中以程序內方式啟動完整 Runtime、注入 fake Clock／ProcessControl／AgentAdapter／QuotaObserver／WorkspaceFs／PlatformInfo，並透過真實 HTTP API 操作；harness 支援「丟棄 Runtime 實例、保留資料目錄後重建」以模擬重啟。現有 `skills/` 目錄與 README 不動（spec D-03）。

**Blocked by:** None（可立即開始）

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [x] 舊版 skill 已以 git tag 封存，`skills/` 與 README 內容未改變
- [x] 一條指令可建置 Runtime、Web UI 與 Tauri App；開發者文件說明建置與測試方式
- [x] 桌面 App 開啟後狀態列顯示 Runtime 狀態；Runtime 未啟動時由 App 啟動，已啟動時連回同一個（單實例，具名 mutex）
- [x] Runtime harness 可注入全部 Port 的 fake，並提供重建 Runtime 的輔助；附一個示範測試：健康端點回應、重建後仍回應
- [x] 產品名稱集中於單一常數與安裝設定（spec D-01）
- [x] CI 或本機測試指令可一次跑完 Rust 與前端測試

## Comments
