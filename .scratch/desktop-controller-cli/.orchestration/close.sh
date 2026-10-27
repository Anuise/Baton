#!/usr/bin/env bash
# 用法：close.sh <NN> <驗證 run id>
# 驗證通過後：合併票分支到 cli/v1、勾選驗收條件、Status 改 completed、記帳並移除 worktree。須在 repo 根目錄、cli/v1 上執行。
set -eu
NN="$1"; VRUN="$2"
O=.scratch/desktop-controller-cli/.orchestration
I=.scratch/desktop-controller-cli/issues
TICKET="$(cd $I && ls $NN-*.md)"
BRANCH="cli/${TICKET%.md}"
WT="$(cd .. && pwd)/issue-orchestrator-wt/cli-$NN"
NOW="$(date '+%Y-%m-%d %H:%M')"
git merge --no-ff -q -m "merge: 票 $NN（驗證 $VRUN 通過）" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" "$BRANCH"
sed -i -e "s/^Status: in_progress（.*）/Status: completed/" -e 's/^- \[ \] /- [x] /' "$I/$TICKET"
sed -i "s/^| $VRUN | \(.*\) | | |$/| $VRUN | \1 | $NOW | PASS |/" "$O/runs.md"
# 舊格式的 Worker 條目補上審閱行
python -I .scratch/platform-rebuild-v1/.orchestration/normalize_decisions.py .scratch/desktop-controller-cli/ai-decisions.md
git add "$I/$TICKET" "$O/runs.md" "$O/runs" .scratch/desktop-controller-cli/ai-decisions.md
git commit -q -m "chore(scratch): 票 $NN 結案" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
# node_modules／target 路徑過長時 git 刪不掉，改用 PowerShell 長路徑刪除
git worktree remove --force "$WT" 2>/dev/null || true
if [ -d "$WT" ]; then
  # 反斜線經 MSYS 傳給 PowerShell 會被吃掉，改以 [char]92 組出 \\?\ 前綴
  WTP="$(cygpath -w "$WT")" powershell -NoProfile -Command '$b=[string][char]92; Remove-Item -LiteralPath ($b+$b+"?"+$b+$env:WTP) -Recurse -Force -Confirm:$false'
fi
git worktree prune
echo "票 $NN 已結案"
