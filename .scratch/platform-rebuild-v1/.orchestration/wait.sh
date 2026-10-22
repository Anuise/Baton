#!/usr/bin/env bash
# 用法：wait.sh <run id>
# 主控 process 重啟後，dispatch.sh 的背景任務會失聯；以此等待既有 run 的紀錄出現最終 result 事件再印出結果。
set -u
LOG="$(cd "$(dirname "$0")" && pwd)/runs/$1.log"
until tail -1 "$LOG" | grep -q '"type":"result"'; do sleep 30; done
tail -1 "$LOG" | python -I -c "import sys,json; d=json.loads(sys.stdin.read()); print('subtype:', d.get('subtype'), 'is_error:', d.get('is_error')); print(d.get('result',''))"
