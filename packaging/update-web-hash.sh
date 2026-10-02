#!/bin/sh
# Rewrites the web UI's dependency hash in nix/web.nix (plan 7e-ii-b), for
# after a change to web/pnpm-lock.yaml. It builds the dependencies with a
# placeholder hash, takes the hash Nix reports, and writes it back. On any
# other failure nix/web.nix is left as it was.
#
# Usage: sh packaging/update-web-hash.sh   (from anywhere in the repo)
set -eu

cd "$(dirname "$0")/.."
file=nix/web.nix
fake=sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=
old=$(sed -n 's/^ *hash = "\(sha256-[A-Za-z0-9+/=]*\)";$/\1/p' "$file")
case $old in
    sha256-*) ;;
    *) echo "FAIL: no single hash line in $file" >&2; exit 1 ;;
esac
[ "$(printf '%s\n' "$old" | wc -l)" -eq 1 ] || { echo "FAIL: more than one hash line in $file" >&2; exit 1; }

backup=$(mktemp)
log=$(mktemp)
cp "$file" "$backup"
# Interrupted, the file gets its old hash back, never the placeholder.
trap 'cp "$backup" "$file"; rm -f "$backup" "$log"; exit 130' INT TERM
trap 'rm -f "$backup" "$log"' EXIT
write() {
    sed "s|\"$old\"|\"$1\"|" "$backup" > "$file.tmp" && mv "$file.tmp" "$file"
}

write "$fake"
# Exit status ignored: with the placeholder the build must fail.
nix build .#web.pnpmDeps --no-link > "$log" 2>&1 || true
new=$(sed -n 's/^ *got: *\(sha256-[A-Za-z0-9+/=]*\)$/\1/p' "$log")
case $new in
    sha256-*) ;;
    *)
        cp "$backup" "$file"
        echo "FAIL: the dependencies did not build; $file is unchanged:" >&2
        tail -n 20 "$log" >&2
        exit 1
        ;;
esac
write "$new"
if [ "$new" = "$old" ]; then
    echo "ok: the hash was already right ($new)"
else
    echo "ok: $old -> $new in $file"
fi
