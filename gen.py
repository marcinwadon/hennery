#!/usr/bin/env python3
"""Expand {{file:REV:PATH:LANG}} markers in the plan template with the file
at that revision of the worktree, fenced; {{text:...}} not used.

Usage: gen.py <template> <out>
"""
import re
import subprocess
import sys

REPO = "/Users/marcinwadon/Projects/marcinwadon/hennery/.claude/worktrees/distribution-7e2a"


def show(rev, path):
    return subprocess.run(
        ["git", "-C", REPO, "show", f"{rev}:{path}"], check=True, capture_output=True, text=True
    ).stdout


def fence(body, lang, indent):
    ticks = "````" if "```" in body else "```"
    if not body.endswith("\n"):
        raise SystemExit("file without final newline")
    text = f"{ticks}{lang}\n{body}{ticks}\n"
    return "".join(indent + l if l.strip() else l for l in text.splitlines(keepends=True))


def main():
    tpl = open(sys.argv[1]).read()

    def repl(m):
        indent, rev, path, lang = m.group(1), m.group(2), m.group(3), m.group(4)
        # The marker's own indent starts the first line already.
        return fence(show(rev, path), lang, indent)

    def tail(m):
        indent, rev, base, path, lang = m.groups()
        new, old = show(rev, path), show(base, path)
        if not new.startswith(old):
            raise SystemExit(f"{path}: {rev} does not extend {base}")
        appended = new[len(old):]
        # "Append" adds a blank line first: the appended text starts after it.
        if not appended.startswith("\n"):
            raise SystemExit(f"{path}: the appended text does not start with a blank line")
        return fence(appended[1:], lang, indent)

    out = re.sub(r"^( *)\{\{tail:([^:]+):([^:]+):([^:]+):([^}]*)\}\}\n", tail, tpl, flags=re.M)
    out = re.sub(r"^( *)\{\{file:([^:]+):([^:]+):([^}]*)\}\}\n", repl, out, flags=re.M)
    open(sys.argv[2], "w").write(out)


if __name__ == "__main__":
    main()
