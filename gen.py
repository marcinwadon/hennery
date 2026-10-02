#!/usr/bin/env python3
"""Expand the plan template's markers with code from the build branch.

Usage: gen.py <template> <out>

Markers, each on a line of its own (its indent is kept):
- {{file:REV:PATH:LANG}}: the whole file at REV, fenced.
- {{hunks:BASE:REV:PATH:LANG}}: "In `PATH`, replace:" / "with:" pairs that
  turn the file at BASE into the file at REV, each old block widened by
  whole lines of context until it occurs exactly once in the file as it
  stands when that pair is applied (pairs are applied in order).
"""
import difflib
import re
import subprocess
import sys

REPO = "/Users/marcinwadon/Projects/marcinwadon/hennery/.claude/worktrees/distribution-7e2a"


def show(rev, path):
    rev = REVS.get(rev, rev)
    return subprocess.run(
        ["git", "-C", REPO, "show", f"{rev}:{path}"], check=True, capture_output=True, text=True
    ).stdout


def fence(body, lang, indent):
    ticks = "````" if "```" in body else "```"
    if not body.endswith("\n"):
        raise SystemExit("block without final newline")
    text = f"{ticks}{lang}\n{body}{ticks}\n"
    return "".join(indent + l if l.strip() else l for l in text.splitlines(keepends=True))


def hunks(base, rev, path, lang, indent):
    old = show(base, path).splitlines(keepends=True)
    new = show(rev, path).splitlines(keepends=True)
    ops = [op for op in difflib.SequenceMatcher(None, old, new, autojunk=False).get_opcodes() if op[0] != "equal"]
    out = []
    current = list(old)
    shift = 0  # how far `current` has moved from `old` before this hunk
    for tag, i1, i2, j1, j2 in ops:
        a, b = i1 + shift, i2 + shift
        replacement = new[j1:j2]
        ctx = 1
        while True:
            lo, hi = max(0, a - ctx), min(len(current), b + ctx)
            before = "".join(current[lo:hi])
            if before and "".join(current).count(before) == 1:
                break
            if lo == 0 and hi == len(current):
                raise SystemExit(f"{path}: no unique context")
            ctx += 1
        after = "".join(current[lo:a] + replacement + current[b:hi])
        out.append(f"{indent}In `{path}`, replace:\n\n" + fence(before, lang, indent) + f"\n{indent}with:\n\n" + fence(after, lang, indent))
        current[a:b] = replacement
        shift += (j2 - j1) - (i2 - i1)
    if "".join(current) != "".join(new):
        raise SystemExit(f"{path}: hunks do not rebuild the file")
    return "\n".join(out)


TASK_OF = [("unpack-", 1), ("check-", 1), ("codex-", 1), ("claude-", 1), ("rust-", 2), ("eval-", 3), ("hm-", 3), ("vm-", 3)]


def code(text):
    text = text.replace("\n", " ⏎ ").strip() or "(nothing)"
    ticks = "``" if "`" in text else "`"
    pad = " " if ticks == "``" else ""
    return f"{ticks}{pad}{text}{pad}{ticks}"


def probes(task, indent):
    import importlib.util
    spec = importlib.util.spec_from_file_location("probes", __file__.replace("gen.py", "probes.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    lines = []
    for p in mod.P:
        t = next(n for prefix, n in TASK_OF if p["name"].startswith(prefix))
        if t != task:
            continue
        olds = p["old"] if isinstance(p["old"], list) else [p["old"]]
        news = p["new"] if isinstance(p["new"], list) else [p["new"]]
        change = "; and ".join(f"{code(o)} → {code(n)}" for o, n in zip(olds, news))
        cmd = " ".join(p["cmd"])
        if "--impure" in p["cmd"]:
            cmd = "NIXPKGS_ALLOW_UNFREE=1 " + cmd
        must = "fail" if p["ends"] == "fail" else "succeed"
        where = "Linux (CI)" if p["where"] == "linux" else "this Mac"
        lines.append(f"{indent}- **{p['name']}** ({where}), in `{p['path']}`: {change}. Run {code(cmd)}: it must {must}, its output matching {code(p['expect'])}.\n")
    if not lines:
        raise SystemExit(f"no probes for task {task}")
    return "".join(lines)


REVS = {
    "BASE": "ecc50cd",
    "B1": "scratch/7e2a-build~4",
    "B2": "scratch/7e2a-build~3",
    "B3": "scratch/7e2a-build~2",
    "B4": "scratch/7e2a-build~1",
    "B5": "scratch/7e2a-build",
}


def main():
    tpl = open(sys.argv[1]).read()
    tpl = re.sub(r"^( *)\{\{probes:(\d+)\}\}\n", lambda m: probes(int(m.group(2)), m.group(1) or "  "), tpl, flags=re.M)
    out = re.sub(
        r"^( *)\{\{hunks:([^:]+):([^:]+):([^:]+):([^}]*)\}\}\n",
        lambda m: hunks(m.group(2), m.group(3), m.group(4), m.group(5), m.group(1)),
        tpl,
        flags=re.M,
    )
    out = re.sub(
        r"^( *)\{\{file:([^:]+):([^:]+):([^}]*)\}\}\n",
        lambda m: fence(show(m.group(2), m.group(3)), m.group(4), m.group(1)),
        out,
        flags=re.M,
    )
    open(sys.argv[2], "w").write(out)


if __name__ == "__main__":
    main()
