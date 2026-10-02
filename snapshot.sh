#!/bin/bash
# Commit this tools directory to branch scratch/7e2a-tools (no checkout),
# via a temporary index, and push it. Usage: snapshot.sh "message"
set -euo pipefail
W=/Users/marcinwadon/Projects/marcinwadon/hennery/.claude/worktrees/distribution-7e2a
T=$W/.superpowers/sdd/7e2a
export GIT_INDEX_FILE=$(mktemp -u /tmp/7e2a-index.XXXXXX)
trap 'rm -f "$GIT_INDEX_FILE"' EXIT
cd "$T"
git --git-dir="$(git -C $W rev-parse --git-common-dir)" --work-tree="$T" add -A -f .
tree=$(git --git-dir="$(git -C $W rev-parse --git-common-dir)" write-tree)
parent=$(git -C $W rev-parse -q --verify refs/heads/scratch/7e2a-tools || true)
commit=$(git -C $W commit-tree "$tree" ${parent:+-p "$parent"} -m "${1:-wip: 7e-ii-a tools}")
git -C $W update-ref refs/heads/scratch/7e2a-tools "$commit"
[ "$(git -C $W log -1 --format=%ae "$commit")" = marcin.wadon@gmail.com ] || { echo "wrong author"; exit 1; }
git -C $W push -q origin scratch/7e2a-tools
echo "tools at $commit"
