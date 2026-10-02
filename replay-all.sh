#!/bin/bash
# Replay plan 7e-ii-a from its own text onto ecc50cd, task by task, and
# compare each task's tree with the build branch's commit for it.
# Usage: replay-all.sh <plan.md>
set -u
W=/Users/marcinwadon/Projects/marcinwadon/hennery/.claude/worktrees/distribution-7e2a
T=$W/.superpowers/sdd/7e2a
R=/tmp/7e2a-replay
PLAN=$1
rm -rf $R; git clone -q $W $R; cd $R || exit 1
git checkout -q -b replay ecc50cd
status=0
for n in 1 2 3 4 5; do
  rev=$(git -C $W rev-parse "scratch/7e2a-build~$((5 - n))")
  python3 $T/replay.py "$PLAN" . $n || { echo "REPLAY FAILED task $n"; status=1; break; }
  git add -A
  if git diff --cached --quiet "$rev"; then echo "MATCH task $n ($rev)"; else echo "DIFF task $n ($rev)"; git diff --cached --stat "$rev"; status=1; fi
  git -c commit.gpgsign=false -c user.email=replay@invalid -c user.name=replay commit -qm "replay task $n"
done
echo "REPLAY DONE status=$status"
exit $status
