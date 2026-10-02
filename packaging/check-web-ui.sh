#!/bin/sh
# The binary embeds the real web UI (plan 4b): a build without it embeds a
# placeholder page, and only that page carries this marker. The collector
# image copies the musl binary, so this covers the image too.
#
# Usage: check-web-ui.sh <binary>
set -eu

binary=$1
marker=hennery-web-ui-not-embedded
[ -f "$binary" ] || { echo "FAIL: no binary at $binary" >&2; exit 1; }
# grep: 0 found, 1 not found, anything else an error (an unreadable file
# must not pass as "not found").
found=0
LC_ALL=C grep -a -q "$marker" "$binary" || found=$?
if [ "$found" -eq 0 ]; then
    echo "FAIL: $binary embeds the placeholder, not the web UI" >&2
    exit 1
fi
if [ "$found" -ne 1 ]; then
    echo "FAIL: could not read $binary" >&2
    exit 1
fi
echo "ok: $binary embeds the web UI"
