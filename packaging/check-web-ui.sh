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
if LC_ALL=C grep -a -q "$marker" "$binary"; then
    echo "FAIL: $binary embeds the placeholder, not the web UI" >&2
    exit 1
fi
echo "ok: $binary embeds the web UI"
