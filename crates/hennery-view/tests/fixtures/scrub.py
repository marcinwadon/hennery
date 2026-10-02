"""Turn a recorded session stream into a golden-test fixture.

Usage: python3 scrub.py <sse dump> <out.jsonl>

Reads the `event: event` messages of an SSE dump of
`GET /api/stream/sessions/{id}` and writes one stored event (`EventDto`) per
line, with every value that could identify a machine, a person or a run
replaced. The same input always gets the same replacement, so ids that tie
events together (`toolCallId`, `pending_id`, `turn_id`) still do.

What it replaces, in every string, key and value alike:
- the run's temporary directories: `/private/tmp/hennery-smoke-<n>` becomes
  `/tmp/smoke`, the agent's own `/private/tmp/claude-<uid>` becomes
  `/tmp/agent`, and their dash-encoded forms (`-private-tmp-...`) likewise;
- every UUID, the ones the dump already part-redacted (`<CODE>`) included,
  by a fresh one numbered in order of first appearance;
- the adapters' message and tool ids (`msg_`, `rs_`, `toolu_`, `call_`), by
  numbered ones of the same prefix;
- every 40-digit hex string (git commits), by a numbered one;
- the account's model note (` · Org default`), removed.
It keeps the 64-digit image hash: that of an 8x8 test image, sent by the test.
"""

import json
import re
import sys

# A UUID, or one the dump part-redacted, where `<CODE>` stands for one or
# two of its groups.
UUID = re.compile(r"(?<![0-9A-Za-z])[0-9a-f]{8}(?:-(?:[0-9a-f]{4}|[0-9a-f]{12}|<CODE>)){2,4}(?![0-9A-Za-z])")
PREFIXED = re.compile(r"\b(msg|rs|toolu|call)_[A-Za-z0-9]+\b")
SHA1 = re.compile(r"\b[0-9a-f]{40}\b")
PATHS = [
    (re.compile(r"/private/tmp/hennery-smoke-[0-9]+"), "/tmp/smoke"),
    (re.compile(r"/private/tmp/claude-[0-9]+"), "/tmp/agent"),
    (re.compile(r"-private-tmp-hennery-smoke-[0-9]+"), "-tmp-smoke"),
    (re.compile(r" · Org default"), ""),
]

seen = {}


def numbered(kind, value, render):
    key = (kind, value)
    if key not in seen:
        seen[key] = render(sum(1 for k in seen if k[0] == kind) + 1)
    return seen[key]


def scrub_str(s):
    for pattern, replacement in PATHS:
        s = pattern.sub(replacement, s)
    s = UUID.sub(lambda m: numbered("uuid", m.group(0), lambda n: f"00000000-0000-7000-8000-{n:012x}"), s)
    s = PREFIXED.sub(lambda m: numbered(m.group(1), m.group(0), lambda n: f"{m.group(1)}_{n:04}"), s)
    s = SHA1.sub(lambda m: numbered("sha1", m.group(0), lambda n: f"{n:040x}"), s)
    return s


def scrub(value):
    if isinstance(value, str):
        return scrub_str(value)
    if isinstance(value, list):
        return [scrub(v) for v in value]
    if isinstance(value, dict):
        return {scrub_str(k): scrub(v) for k, v in value.items()}
    return value


def main(src, dst):
    lines = open(src, encoding="utf-8").read().split("\n")
    out = []
    for at, line in enumerate(lines):
        if line == "event: event" and lines[at + 1].startswith("data: "):
            out.append(scrub(json.loads(lines[at + 1][len("data: "):])))
    with open(dst, "w", encoding="utf-8") as f:
        for event in out:
            f.write(json.dumps(event, ensure_ascii=False, separators=(",", ":")) + "\n")
    print(f"{dst}: {len(out)} events", file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
