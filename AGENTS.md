## Agent skills

### Issue tracker

Issues live as local markdown files under `.scratch/<feature>/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`), recorded as a `Status:` line. See `docs/agents/triage-labels.md`.

### Feature 執行方式：platform-rebuild-v1

實作 `.scratch/platform-rebuild-v1/` 的票之前，必須先讀該目錄下的 `execution.md` 與 `ai-decisions.md`。子代理一律以 `claude -p` 指定 Work-n 帳號執行；**Work-0 是主帳號，不可用來執行 `claude -p`**。查詢各帳號額度一律執行 `python -I .scratch/platform-rebuild-v1/.orchestration/usage.py`，不要用 `claude -p "/usage"`。遇到問題先自行決定，再追加到 `ai-decisions.md` 並標記重要度（高／中／低）。

### Feature 執行方式：desktop-controller-cli

實作 `.scratch/desktop-controller-cli/` 的票之前，必須先讀該目錄下的 `execution.md` 與 `ai-decisions.md`。帳號、額度查詢與 Work-0 的規則與 platform-rebuild-v1 相同；整合分支是 `cli/v1`，計算帳號進行中 run 數時要把兩個 feature 的 `runs.md` 加總。

### Domain docs

Single-context: one `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
