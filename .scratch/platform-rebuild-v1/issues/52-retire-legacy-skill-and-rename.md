# 52: 移除舊 skill、改名 repo 與改寫 README

Status: ready-for-human
Blocked by: 51

**What to build:** 確認新平台可用後：移除舊 issue-orchestrator 與 product-design skill（或移到其他 repo，由使用者決定），repo 依最終產品名改名，改寫 README 為新平台的安裝與使用說明，把 `rebuild/v1` 合併回 main，並移除 AGENTS.md 中本 feature 專屬的執行方式段落。改名與刪除不可逆，因此由人執行或核准。

**Blocked by:** 51 Windows 平台與原生整合驗收

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 產品名稱最終確認，程式內常數、安裝設定與資料目錄名稱一致
- [x] README 描述新平台；舊 skill 的去向已記錄
- [ ] main 包含完整的新平台，issue #3 可由使用者決定是否關閉

## Comments

- 2026-10-27 主控：README 已改寫為 Baton 的說明（commit 5437d0c）。使用者決定直接移除舊 skill、不移到其他 repo：已從 main 刪除 `skills/`（issue-orchestrator 與 product-design）與只記錄舊 skill 的 `docs/validation.md`，最後版本留在 git 歷史與 tag `archive/legacy-skill`（目前只在本機，尚未推送到 origin）。產品與 repo 改名、移除 AGENTS.md 的 feature 執行方式段落仍待處理。
