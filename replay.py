#!/usr/bin/env python3
"""Replay a plan's code blocks onto a tree, task by task.

Usage: replay.py <plan.md> <tree> <task number> [--step1]

Applies the blocks of one task (or only its Step 1 with --step1) per the
plan's "Reading the steps". Prints the number of blocks applied; fails if
none, or if a "replace" block does not occur exactly once.
"""

import re
import sys
from pathlib import Path

INSTR = re.compile(
    r"^(Create|Replace the whole of|Append to|In) `([^`]+)`(?:, replace| with)?:\s*$"
)
FENCE = re.compile(r"^(`{3,})(\S*)\s*$")


def blocks(lines, i):
    """The fenced block starting at or after line i: (text, next index)."""
    while i < len(lines) and not FENCE.match(lines[i]):
        if lines[i].strip():
            raise SystemExit(f"line {i+1}: expected a code block, got {lines[i]!r}")
        i += 1
    m = FENCE.match(lines[i])
    fence = m.group(1)
    j = i + 1
    body = []
    while lines[j].rstrip() != fence:
        body.append(lines[j])
        j += 1
    return "".join(body), j + 1


def task_lines(plan, task, step1):
    text = plan.splitlines(keepends=True)
    start = next(i for i, l in enumerate(text) if l.startswith(f"### Task {task}:"))
    end = next(
        (i for i in range(start + 1, len(text)) if text[i].startswith("### Task ") or text[i].startswith("## ")),
        len(text),
    )
    lines = text[start:end]
    if step1:
        s = next(i for i, l in enumerate(lines) if l.startswith("- [ ] **Step 1"))
        e = next(i for i in range(s + 1, len(lines)) if lines[i].startswith("- [ ] **Step 2"))
        lines = lines[s:e]
    return lines


def main():
    plan_path, tree, task = sys.argv[1], Path(sys.argv[2]), int(sys.argv[3])
    step1 = "--step1" in sys.argv
    plan = Path(plan_path).read_text()
    # Blocks inside a step are indented by the list: strip a common indent.
    lines = [re.sub(r"^  ", "", l) for l in task_lines(plan, task, step1)]
    applied = 0
    i = 0
    while i < len(lines):
        m = INSTR.match(lines[i].strip())
        if not m:
            i += 1
            continue
        kind, path = m.group(1), tree / m.group(2)
        body, i = blocks(lines, i + 1)
        if kind == "Create":
            if path.exists():
                raise SystemExit(f"create: {path} exists")
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        elif kind == "Replace the whole of":
            path.write_text(body)
        elif kind == "Append to":
            old = path.read_text()
            path.write_text(old + "\n" + body)
        else:
            # "In `path`, replace:" then the block, "with:", the block.
            while not lines[i].strip():
                i += 1
            if lines[i].strip() != "with:":
                raise SystemExit(f"line {i+1}: expected 'with:', got {lines[i]!r}")
            new, i = blocks(lines, i + 1)
            old = path.read_text()
            n = old.count(body)
            if n != 1:
                raise SystemExit(f"replace in {path}: found {n} times:\n{body}")
            path.write_text(old.replace(body, new))
        applied += 1
    if applied == 0:
        raise SystemExit("no block applied")
    print(f"task {task}{' step 1' if step1 else ''}: {applied} blocks")


if __name__ == "__main__":
    main()
