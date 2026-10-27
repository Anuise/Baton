#!/usr/bin/env bash
# 用法：prep.sh <impl|verify|fix> <NN> <run id> <Work 編號> [驗證 run id（fix 用）]
# 產生 run 指示檔並在 runs.md 加一列；impl 另外認領票面並建立 worktree。須在 repo 根目錄、rebuild/v1 上執行。
set -eu
KIND="$1"; NN="$2"; RUN="$3"; N="$4"; VRUN="${5:-}"
O=.scratch/platform-rebuild-v1/.orchestration
I=.scratch/platform-rebuild-v1/issues
TICKET="$(cd $I && ls $NN-*.md)"
BRANCH="rebuild/${TICKET%.md}"
WT="$(cd .. && pwd)/issue-orchestrator-wt/$NN"
NOW="$(date '+%Y-%m-%d %H:%M')"
sed -e "s|{{RUN}}|$RUN|g" -e "s|{{NN}}|$NN|g" -e "s|{{BRANCH}}|$BRANCH|g" -e "s|{{TICKET}}|$TICKET|g" \
    -e "s|{{VERIFY_RUN}}|$VRUN|g" -e "s|{{ORCH}}|$(pwd)/$O|g" "$O/$KIND.template.md" > "$O/runs/$RUN.prompt.md"
echo "| $RUN | $NN | $KIND | Work-$N | $NOW | | |" >> "$O/runs.md"
if [ "$KIND" = impl ]; then
  sed -i "s/^Status: \(ready-for-agent\|in_progress（.*）\)/Status: in_progress（Work-$N，run $RUN，$NOW）/" "$I/$TICKET"
  git add "$I/$TICKET" "$O/runs.md" "$O/runs/$RUN.prompt.md"
  git commit -q -m "chore(scratch): 認領票 $NN（$RUN）" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
  if [ -d "$WT" ]; then git -C "$WT" merge -q rebuild/v1; else git worktree add -q -b "$BRANCH" "$WT" rebuild/v1; fi
fi
echo "$RUN 已準備：$WT"
