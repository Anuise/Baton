# 執行方式：實作 platform-rebuild-v1

本檔規定實作本 spec 與 `issues/` 時怎麼派出子代理。執行本 feature 的主控 session 每一輪開頭都要讀這份檔案與 [ai-decisions.md](ai-decisions.md)。

標 **〔使用者指定〕** 的條目是使用者明確要求的；標 **〔AI-NN〕** 的條目是 AI 代為決定的，理由與重要度記在 ai-decisions.md。

## 帳號

- 本機的 Claude Code 帳號以 `Work-n` 命名，原生設定目錄為使用者家目錄下的 `.claude-Work-n`。2026-10-22 盤點到的有 Work-0、Work-1、Work-3、Work-8、Work-9。〔使用者指定〕
- **Work-0 是主帳號（也就是主控 session 使用的帳號），永遠不能用來執行 `claude -p`。** 〔使用者指定〕
- 可派工的帳號是 `.claude-Work-*` 目錄中、排除 Work-0 後的全部帳號；每次派工前重新列目錄，不寫死名單。〔AI-03〕
- 子代理一律透過 `claude -p` 指定 Work-n 執行，不使用主控 session 內建的 Agent／subagent 工具做實作、驗證或審查，避免用到主帳號的額度。〔使用者指定〕主控 session 自己只能做讀檔、排程、記帳和寫票面中繼資料這類輕量工作。〔AI-04〕

## 派出前檢查

1. **一律用 `python -I .scratch/platform-rebuild-v1/.orchestration/usage.py` 查詢額度**（不消耗額度），不要再用 `claude -p "/usage"`。〔使用者指定，見 AI-20〕腳本每個帳號印一行 `<n> | <5h 已用>% (reset …) | <7d 已用>% (reset …)`：
   - 5h 剩餘 = 100 − 第二欄
   - 7d 剩餘 = 100 − 第三欄
2. **5h 剩餘 ≥ 3 且 7d 剩餘 ≥ 3** 才可以派出。〔使用者指定；「≥3」解讀為剩餘百分比，見 AI-01〕
3. 該行顯示 `err …` 或解析不到數字時，視為該帳號不可用，改用下一個帳號。〔AI-05〕
4. 該帳號目前進行中的 run 必須 < 3。〔使用者指定〕

## 啟動參數

每個 run 都以下列條件啟動：

| 項目 | 值 | 來源 |
| --- | --- | --- |
| 帳號 | `CLAUDE_CONFIG_DIR` 指向 `.claude-Work-n` | 使用者指定 |
| 環境清理 | 先移除從主控 session 繼承的所有 `CLAUDE*`、`CLAUDECODE`、`ANTHROPIC*`、`AI_AGENT`、`BAGGAGE` 環境變數，再設定 `CLAUDE_CONFIG_DIR` | AI-02 |
| 模型 | `--model claude-sonnet-5-5` | 使用者指定 |
| effort | `--effort high` | 使用者指定 |
| 限流等待 | `CLAUDE_CODE_RETRY_WATCHDOG=1`、`CLAUDE_CODE_RETRY_WATCHDOG_MAX_USAGE_LIMIT_WAIT_MS=18000000`（5 小時） | 使用者指定；變數名稱見 AI-07 |
| 權限 | `--permission-mode bypassPermissions`，工作目錄限定在該票的 worktree | AI-08 |
| 輸出 | `--output-format stream-json --verbose`，完整輸出寫入 run 紀錄檔 | AI-09 |
| 工作目錄 | 每張票一個 git worktree 與分支 | AI-10 |

範例形狀（Git Bash；`$CLEAN` 是由上述環境清理規則產生的 `env -u …` 參數清單）：

```bash
MSYS_NO_PATHCONV=1 env $CLEAN \
  CLAUDE_CONFIG_DIR="$HOME/.claude-Work-3" \
  CLAUDE_CODE_RETRY_WATCHDOG=1 \
  CLAUDE_CODE_RETRY_WATCHDOG_MAX_USAGE_LIMIT_WAIT_MS=18000000 \
  claude -p --model claude-sonnet-5-5 --effort high \
    --permission-mode bypassPermissions \
    --output-format stream-json --verbose \
    "$(cat <run 指示檔>)" > <run 紀錄檔> 2>&1
```

主控要在背景啟動 run，並在 run 結束時收到通知，不要用輪詢的方式等待。

## 流程

1. 讀票：`issues/` 中 `Status: ready-for-agent`、而且 `Blocked by:` 列出的票都已 `completed` 的票，就是可派的 frontier。依編號由小到大選。〔AI-11〕
2. 認領：把票面改成 `Status: in_progress`，並記錄帳號、run 編號與時間。建立 worktree。
3. 實作 run：派一個 Worker（`claude -p`）。指示中要附上 spec 路徑、票路徑、執行方式檔、ai-decisions.md，並要求 Worker 以 TDD 方式實作、commit 到該票的分支，遇到不明確之處自行決定並寫進 ai-decisions.md。
4. 驗證 run：派一個**不同的** `claude -p` run 做唯讀驗證；可以用同一個帳號，但必須是新的 run。驗證者逐條核對票的驗收條件並跑測試，產出通過或不通過的結論與證據。〔AI-12〕
5. 通過就合併回整合分支，勾選票面的驗收條件，改成 `Status: completed`；不通過就派新的修正 run。每張票最多修正 3 次，超過就改成 `Status: blocked`，寫明原因後跳到下一張。〔AI-13〕
6. 帳號撞牆時，CLI 會依 watchdog 設定自行等待，最多 5 小時；超過 5 小時 run 會失敗結束。失敗的 run 不計入修正次數，換一個帳號、以新的 run 接手。〔AI-14〕

## 記帳

run 紀錄放在 `.scratch/platform-rebuild-v1/.orchestration/` 底下：每個 run 一份指示檔、一份紀錄檔，再加一份 `runs.md` 總表，記錄帳號、票號、開始與結束時間、結果。開新 run 前，用總表計算每個帳號進行中的 run 數。〔AI-15〕

## 決策紀錄

過程中遇到問題，一律先自己決定、繼續做，再依 [ai-decisions.md](ai-decisions.md) 的格式追加紀錄，標上重要度（高／中／低）。〔使用者指定〕Worker 也遵守這條規則，但只能追加新條目，不能修改既有條目。
