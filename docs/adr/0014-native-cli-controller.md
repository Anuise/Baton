# ADR-0014：以原生互動式 Claude Code 作為 Controller

- 來源識別：desktop-controller-cli Q1、Q2、Q8、Q9
- 狀態：**AI 暫定，待使用者確認**（改動成本：高）
- 來源：[.scratch/desktop-controller-cli/grilling.md](../../.scratch/desktop-controller-cli/grilling.md)
- 相關：取代 [ADR-0009](0009-controller-role-and-event-driven-turns.md) 的 A1（事件驅動喚醒）與 B1（對話依序處理）；沿用 [ADR-0013](0013-local-mcp-tool-interface.md) 的 MCP shim 與 Agent 憑證

## 背景

ADR-0009 讓 Controller 以「每輪一次 `claude -p`＋resume」運作：使用者在桌面 App 的對話頁打字，Runtime 合併事件後喚醒一輪，前端輪詢結果。實際使用後，使用者覺得這種對話方式很難用：看不到 Controller 的工具呼叫過程，不能用 Claude Code 原生的指令、skills 與快捷鍵，回應還要等事件合併與輪詢。

另一方面，使用者在 `.scratch/platform-rebuild-v1` 實際跑順的方式是：自己開互動式 Claude Code 當主控，主控在背景以其他帳號跑 `claude -p` 的 Worker，背景程序結束時由 Claude Code 原生通知主控。

## 決策

- **Controller 是使用者直接操作的原生互動式 `claude`**（不加 `-p`）。平台不轉述對話、不自動喚醒 Controller 輪次；移除對話頁與自動喚醒。
- **開在外部終端機視窗**：桌面 App 的 Controller 頁簽讓使用者選一個 Account Profile 並開啟；有 Windows Terminal 就用它，否則用系統預設主控台。
- **開啟＝建立一個互動式 Controller Run**：Runtime 簽發 Controller 範圍的 Agent 憑證，以選定 Profile 的 `CLAUDE_CONFIG_DIR`（先清掉繼承的 Claude／Anthropic 環境變數）、MCP shim 設定、共同設定快照與內建「Controller 操作手冊」（`--append-system-prompt-file`）啟動，工作目錄是 Workspace 目錄。CLI 程序結束＝Run 結束＝憑證失效。每個 Workspace 同時最多一個 Controller CLI。
- **Controller 帳號開啟期間不派 Worker**。
- **事件通知走背景程序**：MCP shim 執行檔提供等待子命令，Controller 以 Claude Code 的背景 Bash 執行；有新事件（Run 結束、受阻、裁定被回答、訊息）時印出摘要並結束，由 Claude Code 原生通知 Controller。憑證從環境變數繼承，不出現在命令列。
- 帳號挑選、額度門檻、並行上限等派工規則仍由 Runtime 負責，不寫進操作手冊。

## 替代方案

- **保留 `claude -p` 輪次、只改善對話 UI**：根本問題（不透明、無原生功能、延遲）不會消失。
- **內嵌終端機（ConPTY＋xterm.js）**：畫面統一，但工作量大一個量級，Claude Code 的 TUI 在內嵌終端常有重繪與中文輸入法問題。
- **MCP 工具長輪詢等待事件**：會卡住整個回合、受工具逾時限制。
- **Runtime 把文字送進終端機**：外部視窗做不到，內嵌也不可靠。

## 後果

- Controller 的品質與連續性取決於使用者是否開著 CLI；沒開時事件只會累積，不會有人處理。
- 不再有 Controller 自動交接（票 30 的機制對互動式 Controller 不適用）；額度不足時由使用者「換帳號重開」，跨帳號不 resume，新 Session 靠平台狀態接續。
- 操作手冊要求 Controller 派工後掛著等待；若 Controller 忘了掛，就收不到通知。
- 真實開窗與 `claude` 互動只能手動驗收；Runtime 側以 harness 與假的終端機 port 測試。
