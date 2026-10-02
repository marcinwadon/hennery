#!/bin/sh
# A release archive holds the `hennery` binary, its licence and its README,
# and nothing else (distribution spec §3.3): never an adapter, nor the Claude
# CLI, which hennery does not redistribute.
#
# Usage: check-archive.sh <archive.tar.xz> <target>
set -eu

archive=$1
dir="hennery-$2"
listed=$(tar -tJf "$archive" | sort)
expected=$(printf '%s\n' "$dir/" "$dir/LICENSE" "$dir/README.md" "$dir/hennery" | sort)
if [ "$listed" != "$expected" ]; then
    echo "FAIL: $archive holds:" >&2
    echo "$listed" >&2
    echo "and should hold exactly:" >&2
    echo "$expected" >&2
    exit 1
fi
echo "ok: $archive holds the binary, its licence and its README alone"
