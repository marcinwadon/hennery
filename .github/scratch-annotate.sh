#!/bin/bash
# Scratch only (plan 7e-ii-a): run a command; on failure, put the tail of its
# output in an error annotation, which the public API shows without a token.
set -o pipefail
"$@" 2>&1 | tee /tmp/step.log
status=$?
if [ "$status" -eq 0 ]; then
  grep -E 'running .*--version|^[a-z0-9-]*> [0-9]+\.[0-9]+|"result"' /tmp/step.log | cut -c1-300 | tail -c 3000 |
    python3 -c 'import sys; d=sys.stdin.read().replace("%","%25").replace("\r","%0D").replace("\n","%0A"); print("::notice title=passed::"+d)'
fi
if [ "$status" -ne 0 ]; then
  # The public page shows an annotation's first few thousand bytes: one
  # for the error lines, then the log's tail in pieces, last piece first.
  grep -vE '(Compiling|Checking|Fresh|searching for dependencies|setting RPATH|setting interpreter|Downloading|Downloaded) ' /tmp/step.log > /tmp/step.short.log
  grep -iE 'error|fail|cannot|refus|assert|Traceback' /tmp/step.short.log | tail -c 3000 > /tmp/step.errors.log
  tail -c 12000 /tmp/step.short.log > /tmp/step.tail.log
  python3 - <<'EOF'
def emit(title, text):
    d = text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
    print(f"::error title={title}::{d}")
emit("errors", open("/tmp/step.errors.log").read())
tail = open("/tmp/step.tail.log").read()
pieces = [tail[i:i + 3000] for i in range(0, len(tail), 3000)]
for n, piece in reversed(list(enumerate(pieces))):
    emit(f"tail {n + 1} of {len(pieces)}", piece)
EOF
fi
exit "$status"
