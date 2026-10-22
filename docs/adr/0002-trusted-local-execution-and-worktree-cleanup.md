# ADR-0002：可信本機執行與 worktree 自動清理

- 來源識別：DR-20261021-02
- 狀態：已確認（使用者核准，2026-10-21）；Windows 原生程序、worktree 清理與恢復為 Validation Required，尚未驗證
- 來源：[issue #3](https://github.com/Anuise/issue-orchestrator/issues/3) §17 DR-20261021-02；§1.2、§3.3、§4.1、§12.2、§13、§15.4
- Spec 對應：[spec.md](../../.scratch/platform-rebuild-v1/spec.md) 的 D-10（程序控制）、User Stories 31、37–39

## 背景

平台要同時驅動多個 Windows 原生 Claude Code。若再建一層作業系統沙箱，會增加第一版的部署與相容成本；若所有 Worker 共用一個 checkout，並行修改會互相衝突；若 worktree 永不清理，磁碟空間會持續被占用。

## 決策

- 平台信任 Claude Code，以目前 Windows 使用者身分直接管理多個 Windows 原生 Claude Code 程序，不另建 OS 沙箱。工作目錄與 worktree 用於組織工作與避免並行衝突，**不是**安全邊界。信任模式不等於關閉 Claude Code 的權限確認，也不增加操作授權。
- 不同開發 Task 使用各自獨立的 worktree，研究與文件 Task 使用獨立輸出目錄。
- Task 通過必要驗證、成果已整合至指定目標，且安全條件滿足後，立即自動移除「平台建立並登記」的 worktree。安全條件：無其他使用者與未知 writer；必要成果、日誌、證據已保存到 worktree 外；無未保存或歸屬不明的檔案；目標確為平台登記的 worktree。
- Task 完成與清理結果分開保存與顯示。暫停、交接、失敗、取消與完全退出不觸發清理。
- 清理失敗時持久化待清理狀態與原因，有限重試，每次重試重新核對安全條件；重啟後先核對登記目標、實際目錄與 Git worktree 登記，再繼續。

本決策只授權 worktree 清理，不包含自動刪除 Git 分支、Git 歷史或正式 Artifact。

Spec 補充（AI 暫定，待使用者確認）：D-10 暫定每個 Run 使用一個 Windows Job Object，程序身分 = PID + 建立時間 + 指令雜湊。

## 替代方案

- 額外 OS 沙箱：增加部署與相容性成本。
- 所有 Worker 共用 checkout：增加並行修改衝突。
- 完成後保留一段時間或手動清理：持續占用空間。

## 後果

- 平台不承諾阻止同一 Windows 使用者程序存取其他資源；移除 worktree 也不會釋放 Git 共用物件與歷史占用的空間。
- Windows 檔案占用等清理失敗必須以可行動的原因呈現，不能把已完成的 Task 改成失敗，也不能顯示為已清理。
- worktree 清理後不得直接在已移除路徑 resume；重開工作時先依已保存成果重建工作副本。
- 手動保留現場、保留期限與空間管理面板留待後續版本。
