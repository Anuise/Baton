# 執行方式：實作 desktop-controller-cli

本檔規定實作本 spec 與 `issues/` 時怎麼派出子代理。執行本 feature 的主控 session 每一輪開頭都要讀這份檔案與 [ai-decisions.md](ai-decisions.md)。

本檔沿用 [platform-rebuild-v1 的執行方式](../platform-rebuild-v1/execution.md)，以下只列出**相同的規則**與**不同之處**。標 **〔使用者指定〕** 的是使用者明確要求的；標 **〔PR-AI-NN〕** 的是 platform-rebuild-v1 的 AI 決策；標 **〔AI-NN〕** 的是本 feature 的 AI 決策，理由與重要度記在本目錄的 ai-decisions.md。

## 與 platform-rebuild-v1 相同的規則

- 帳號：`Work-n`，原生設定目錄是 `~/.claude-Work-n`；**Work-0 是主帳號，永遠不能用來執行 `claude -p`**〔使用者指定〕。可派工帳號每次派工前重新列目錄、排除 Work-0〔PR-AI-03〕。
- 子代理一律以 `claude -p` 指定 Work-n 執行，主控 session 不用內建 Agent／subagent 做實作、驗證或審查〔使用者指定〕；主控只做讀檔、排程、記帳與寫票面中繼資料〔PR-AI-04〕。
- 額度查詢一律 `python -I .scratch/platform-rebuild-v1/.orchestration/usage.py`〔使用者指定〕；**5h 剩餘 ≥ 3 且 7d 剩餘 ≥ 3** 才派；`err` 或解析不到視為不可用；每帳號進行中 run < 3〔使用者指定〕。
- 啟動參數（環境清理、`CLAUDE_CONFIG_DIR`、`--model claude-opus-5-5`、`--effort high`、watchdog 5 小時、`bypassPermissions`、stream-json 紀錄）完全照 `dispatch.sh`。
- 流程：frontier 依編號由小到大 → 認領＋worktree → 實作 run（TDD、最多 3 個 subagent）→ **不同的** run 唯讀驗證 → 通過就合併、勾選、`completed`；不通過派修正 run，**最多 3 次**，超過標 `blocked` 跳下一張 → 撞牆失敗的 run 不計修正次數、換帳號重派。
- 主控在背景啟動 run，run 結束時收到通知，不輪詢；主控重啟後失聯的 run 用 `wait.sh` 等。
- 遇到問題一律先自己決定、繼續做，再追加到 ai-decisions.md 並標重要度〔使用者指定〕；Worker 只能追加、不能修改既有條目。
- **不要在有 run 執行中時修改 `.orchestration/dispatch.sh`。**

## 不同之處

| 項目 | 本 feature | 來源 |
| --- | --- | --- |
| 整合分支 | `cli/v1`，第一次派工前從 `rebuild/v1` 目前的 HEAD 建立 | AI-01 |
| 票分支 | `cli/<票檔名去掉 .md>` | AI-01 |
| 票 worktree | repo 同層的 `issue-orchestrator-wt/cli-<NN>`（與 platform-rebuild-v1 的 `<NN>` 不衝突） | AI-01 |
| 腳本與範本 | `.scratch/desktop-controller-cli/.orchestration/`：由 platform-rebuild-v1 的同名檔複製，只換路徑與分支名；`usage.py`、`normalize_decisions.py` 直接用 platform-rebuild-v1 的 | AI-02 |
| 最終 review 基準 | `cli/v1` 相對 `rebuild/v1` 的差異（`rebuild/v1` 尚未進 `main`） | AI-01 |

## 指令速查（在 repo 根目錄、`cli/v1` 上執行）

```bash
O=.scratch/desktop-controller-cli/.orchestration
python -I .scratch/platform-rebuild-v1/.orchestration/usage.py
bash $O/prep.sh impl 01 R001-01-impl 3
bash $O/dispatch.sh 3 R001-01-impl ../issue-orchestrator-wt/cli-01   # 背景執行
bash $O/prep.sh verify 01 R002-01-verify 8
bash $O/dispatch.sh 8 R002-01-verify ../issue-orchestrator-wt/cli-01  # 背景執行
bash $O/close.sh 01 R002-01-verify
```

驗證 run 的結論要由主控存成 `runs/<驗證 run>.result.md`，修正 run 會讀它。

## 記帳

run 紀錄放在 `.orchestration/` 底下：每個 run 一份指示檔、一份紀錄檔（`*.log` 不進版控），加上 `runs.md` 總表。開新 run 前用總表計算每個帳號進行中的 run 數。**兩個 feature 同時在跑時，帳號的進行中 run 數要把兩份 `runs.md` 加總。**〔AI-03〕

## 平行派工注意

- 票 01（頁簽外殼）與票 02（以既有目錄建立 Profile）都會碰帳號畫面，可平行，但後合併的那張可能需要 `integrate.template.md` 的整合 run。〔AI-04〕
- 票 04～08 都只依賴 03，彼此改到的區域不同，可平行；05 與 07 都會碰 Scheduler／恢復，若衝突走整合 run。
