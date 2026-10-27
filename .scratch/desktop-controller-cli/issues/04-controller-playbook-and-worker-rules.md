# 04: Controller 操作手冊、Worker 通則與 AI 決策紀錄

Status: completed
Blocked by: 03

**What to build:** Controller CLI 一開就載入隨 App 發佈、有版本號的「Controller 操作手冊」（`execution.md` 的平台化版本：frontier、`dispatch_task`、背景 `wait`、獨立驗證、最多 3 次修正、先決定再記錄、只有決策權限表規定的才提 Decision Request、AI 決策紀錄的位置與格式；不含帳號名稱、額度門檻、並行上限、worktree 路徑）。Worker 啟動時附加固定的 Worker 通則（TDD、commit 到 Task 的 worktree 分支、最多 3 個 subagent、AI 決策只能追加）。開啟 Controller 前若 Workspace 沒有 `.baton/ai-decisions.md`，以標頭範本建立。User stories 26、49～63。

**Blocked by:** 03

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)　**ADR：** [ADR-0014](../../../docs/adr/0014-native-cli-controller.md)

## Acceptance criteria

- [x] Controller CLI 的啟動規格以 `--append-system-prompt-file` 帶入 Run 專屬唯讀目錄中的操作手冊；內容與版本號一致
- [x] 操作手冊不含任何帳號名稱、額度數字、並行上限或 worktree 路徑（以測試檢查）
- [x] Controller 頁簽顯示操作手冊版本號
- [x] Worker Run 的系統提示包含 Worker 通則；既有 Worker 啟動行為的測試仍通過
- [x] Workspace 沒有 `.baton/ai-decisions.md` 時開啟 Controller 會建立它（格式與 platform-rebuild-v1 的 ai-decisions.md 檔頭相同）；已存在時不覆寫
- [x] 以 Runtime harness 撰寫情境測試涵蓋上述行為並通過
- [x] AI 決策已追加至 ai-decisions.md

## Comments
