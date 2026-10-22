#!/usr/bin/env bash
# 用法：dispatch.sh <Work 編號> <run id> <工作目錄>
# 以指定 Work 帳號執行 claude -p；指示檔為 runs/<run id>.prompt.md，紀錄寫到 runs/<run id>.log
set -u
ORCH="$(cd "$(dirname "$0")" && pwd)"
N="$1"; RUN="$2"; WD="$3"
[ "$N" = "0" ] && { echo "禁止使用 Work-0"; exit 2; }
[[ "$N" =~ ^[0-9]+$ && "$RUN" =~ ^[A-Za-z0-9_-]+$ ]] || { echo "帳號或 run id 格式不符"; exit 2; }
# 工作目錄只能是本專案的票 worktree（AI-08、AI-10）
WT_ROOT="$(cd "$ORCH/../../../.." && pwd)/issue-orchestrator-wt/"
case "$(cd "$WD" 2>/dev/null && pwd)/" in "$WT_ROOT"?*) ;; *) echo "工作目錄不在 $WT_ROOT 之下"; exit 2;; esac
# 移除從主控 session 繼承的環境變數（AI-02）
CLEAN=""
for v in $(env | cut -d= -f1 | grep -E '^(CLAUDE|ANTHROPIC|AI_AGENT$|BAGGAGE$)'); do CLEAN="$CLEAN -u $v"; done
export PATH="$HOME/.cargo/bin:$PATH"
cd "$WD" || exit 3
MSYS_NO_PATHCONV=1 env $CLEAN \
  CLAUDE_CONFIG_DIR="$(cygpath -w "$HOME/.claude-Work-$N")" \
  CLAUDE_CODE_RETRY_WATCHDOG=1 \
  CARGO_INCREMENTAL=0 \
  CARGO_PROFILE_DEV_DEBUG=0 \
  CLAUDE_CODE_RETRY_WATCHDOG_MAX_USAGE_LIMIT_WAIT_MS=18000000 \
  claude -p --model claude-opus-5-5 --effort high \
    --permission-mode bypassPermissions \
    --output-format stream-json --verbose \
    "$(cat "$ORCH/runs/$RUN.prompt.md")" > "$ORCH/runs/$RUN.log" 2>&1
RC=$?
# 印出最後的 result 事件
grep '"type":"result"' "$ORCH/runs/$RUN.log" | tail -1 | python -I -X utf8 -c "import sys,json; t=sys.stdin.buffer.read().decode('utf-8'); d=json.loads(t) if t.strip() else {}; print('subtype:', d.get('subtype'), 'is_error:', d.get('is_error')); print(d.get('result',''))"
echo "exit: $RC"
