## Agent skills

### Issue tracker

Issues live as local markdown files under `.scratch/<feature>/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`), recorded as a `Status:` line. See `docs/agents/triage-labels.md`.

### Feature 執行方式：platform-rebuild-v1

實作 `.scratch/platform-rebuild-v1/` 的票之前，必須先讀該目錄下的 `execution.md` 與 `ai-decisions.md`。子代理一律以 `claude -p` 指定 Work-n 帳號執行；**Work-0 是主帳號，不可用來執行 `claude -p`**。遇到問題先自行決定，再追加到 `ai-decisions.md` 並標記重要度（高／中／低）。

### Domain docs

Single-context: one `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
