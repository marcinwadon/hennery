#!/bin/sh
# A release binary must run on a machine without our build's libraries
# (distribution spec §1.1): on Linux it is fully static (no `NEEDED` entry,
# no interpreter); on macOS it links only the system's own libraries, not
# Homebrew's or Nix's OpenSSL (plan 7a).
#
# Usage: check-linkage.sh <binary>
set -eu

binary=$1
[ -f "$binary" ] || { echo "FAIL: no binary at $binary" >&2; exit 1; }
case "$(uname -s)" in
Linux)
    dynamic=$(readelf -d "$binary")
    if printf '%s\n' "$dynamic" | grep -q NEEDED; then
        printf '%s\n' "$dynamic"
        echo "FAIL: $binary links shared libraries" >&2
        exit 1
    fi
    program=$(readelf -l "$binary")
    if printf '%s\n' "$program" | grep -q "Requesting program interpreter"; then
        printf '%s\n' "$program"
        echo "FAIL: $binary needs a dynamic loader" >&2
        exit 1
    fi
    ;;
Darwin)
    libs=$(otool -L "$binary")
    # otool exits 0 and prints one line even for a non-Mach-O file ("is not
    # an object file"); a real binary's first line is just "<path>:".
    if [ "$(printf '%s\n' "$libs" | head -n 1)" != "$binary:" ]; then
        printf '%s\n' "$libs"
        echo "FAIL: $binary is not readable by otool as a Mach-O binary" >&2
        exit 1
    fi
    # Every line after the first names a library; only the system's count.
    foreign=$(printf '%s\n' "$libs" | tail -n +2 | grep -v -E '^[[:space:]]+/(usr/lib|System/Library)/' || true)
    if [ -n "$foreign" ]; then
        printf '%s\n' "$libs"
        echo "FAIL: $binary links libraries outside the system:" >&2
        echo "$foreign" >&2
        exit 1
    fi
    ;;
*)
    echo "FAIL: no linkage check for $(uname -s)" >&2
    exit 1
    ;;
esac
echo "ok: $binary links nothing outside the system"
