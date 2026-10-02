#!/bin/sh
# The Homebrew formula names every archive of this build with the archive's
# own SHA-256, on the line after its URL.
#
# Usage: check-formula.sh <directory holding hennery.rb and every archive's
# .sha256>
set -eu

dist=$1
formula="$dist/hennery.rb"
count=0
for sum in "$dist"/*.tar.xz.sha256; do
    archive=$(basename "$sum" .sha256)
    want=$(cut -d ' ' -f 1 "$sum")
    got=$(awk -v archive="/$archive\"" 'found { print; exit } $1 == "url" && index($2, archive) { found = 1 }' "$formula")
    case "$got" in
    *"sha256 \"$want\""*) ;;
    *)
        echo "FAIL: $formula does not give $archive its SHA-256 ($want): $got" >&2
        exit 1
        ;;
    esac
    count=$((count + 1))
done
urls=$(grep -c '^[[:space:]]*url "' "$formula")
if [ "$urls" != "$count" ]; then
    echo "FAIL: $formula names $urls archives, this build made $count" >&2
    exit 1
fi
echo "ok: the formula gives each of its $count archives its SHA-256"
