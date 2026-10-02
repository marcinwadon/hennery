#!/bin/sh
# The hardened shell installer (distribution spec §2, §9), run against the
# archives of this build, served from a directory (`file://`):
#   - each archive of the build is offered, with its SHA-256 embedded;
#   - it installs a binary that runs, with `sha256sum` or with `shasum`;
#   - a checksum mismatch aborts, and installs nothing;
#   - no SHA-256 tool, an empty checksum, no checksum at all, or a checksum
#     style it does not know aborts, and installs nothing.
# HOME and XDG_CONFIG_HOME point into a scratch directory, so nothing outside
# it is written.
#
# Usage: test-installer.sh <directory holding hennery-installer.sh and every
# archive with its .sha256>
set -eu
# Either would install outside the scratch directory and make the "left a
# binary" checks vacuous.
unset HENNERY_INSTALL_DIR CARGO_DIST_FORCE_INSTALL_DIR

dist=$(cd "$1" && pwd)
installer="$dist/hennery-installer.sh"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

# Every archive the installer offers has its checksum embedded, and it is the
# one cargo-dist wrote beside that archive.
arms=$(sed -n 's/^[[:space:]]*"\(hennery-.*\.tar\.xz\)")$/\1/p' "$installer")
[ -n "$arms" ] || fail "the installer offers no archive"
count=0
for archive in $arms; do
    sum="$dist/$archive.sha256"
    [ -f "$sum" ] || fail "the installer offers $archive, and this build has no $archive.sha256"
    want=$(cut -d ' ' -f 1 "$sum")
    case "$want" in
    *[!0-9a-f]* | "") fail "$sum holds no SHA-256" ;;
    esac
    [ "${#want}" = 64 ] || fail "$sum holds no SHA-256"
    # The installer's case for this archive, up to its `;;`.
    arm=$(awk -v name="\"$archive\")" '$1 == name { on = 1 } on { print } on && $1 == ";;" { exit }' "$installer")
    printf '%s\n' "$arm" | grep -qx '[[:space:]]*_checksum_style="sha256"' ||
        fail "the installer has no sha256 checksum for $archive"
    printf '%s\n' "$arm" | grep -qx "[[:space:]]*_checksum_value=\"$want\"" ||
        fail "the installer's checksum for $archive is not the archive's"
    count=$((count + 1))
done
built=0
for archive in "$dist"/*.tar.xz; do
    built=$((built + 1))
done
[ "$count" = "$built" ] || fail "the installer offers $count archives, this build made $built"
echo "ok: the installer embeds each of its $count archives' SHA-256"

# run_installer <download dir> <install dir> <log>: the installer's exit status.
run_installer() {
    mkdir -p "$scratch/home"
    status=0
    HOME="$scratch/home" XDG_CONFIG_HOME="$scratch/home/.config" \
        HENNERY_DOWNLOAD_URL="file://$1" HENNERY_UNMANAGED_INSTALL="$2" \
        sh "$1/hennery-installer.sh" >"$3" 2>&1 || status=$?
    return "$status"
}

# refused <name> <download dir> <message>: the installer, run on <download
# dir>, fails with <message> and leaves no binary.
refused() {
    if run_installer "$2" "$scratch/$1" "$scratch/$1.log"; then
        cat "$scratch/$1.log"
        fail "$1: installed"
    fi
    grep -q "$3" "$scratch/$1.log" || {
        cat "$scratch/$1.log"
        fail "$1: failed for another reason"
    }
    [ ! -e "$scratch/$1/hennery" ] || fail "$1: left a binary"
}

# tools <dir> <excluded>...: every command of /usr/bin and /bin but these.
tools() {
    dir=$1
    shift
    mkdir "$dir"
    for from in /usr/bin /bin; do
        for tool in "$from"/*; do
            name=$(basename "$tool")
            for excluded in "$@"; do
                [ "$name" = "$excluded" ] && continue 2
            done
            [ -e "$dir/$name" ] || ln -s "$tool" "$dir/$name"
        done
    done
}

# copy <name>: a download directory with the installer and the archives.
copy() {
    mkdir "$scratch/$1"
    cp "$installer" "$dist"/*.tar.xz "$scratch/$1/"
    echo "$scratch/$1"
}

UNCHECKED="refusing to install an archive whose checksum cannot be checked"

# A good install.
run_installer "$dist" "$scratch/good" "$scratch/good.log" || {
    cat "$scratch/good.log"
    fail "the installer failed on its own archives"
}
"$scratch/good/hennery" --version || fail "the installed binary does not run"
echo "ok: installs a binary that runs"

# With `shasum` and no `sha256sum`.
tools "$scratch/shasum-tools" sha256sum
# In a subshell: an assignment before a function call may outlive it.
(PATH="$scratch/shasum-tools" && run_installer "$dist" "$scratch/shasum" "$scratch/shasum.log") || {
    cat "$scratch/shasum.log"
    fail "the installer failed with shasum alone"
}
"$scratch/shasum/hennery" --version >/dev/null || fail "the binary installed with shasum does not run"
echo "ok: installs with shasum alone"

# A checksum mismatch: the same archives, one byte longer each.
tampered=$(copy tampered-download)
for archive in "$tampered"/*.tar.xz; do
    printf 'x' >>"$archive"
done
refused tampered "$tampered" "checksum mismatch"
echo "ok: a checksum mismatch aborts"

# No SHA-256 tool at all.
tools "$scratch/no-tools" sha256sum shasum
(PATH="$scratch/no-tools" && refused unchecked "$dist" "$UNCHECKED")
echo "ok: no SHA-256 tool aborts"

# An empty checksum.
empty=$(copy empty-download)
sed 's/^\([[:space:]]*_checksum_value=\)".*"$/\1""/' "$installer" >"$empty/hennery-installer.sh"
refused empty "$empty" "$UNCHECKED"
echo "ok: an empty checksum aborts"

# No checksum at all.
none=$(copy none-download)
grep -v '^[[:space:]]*_checksum_style=' "$installer" >"$none/hennery-installer.sh"
refused none "$none" "$UNCHECKED"
echo "ok: no checksum aborts"

# A checksum style the installer does not know.
unknown=$(copy unknown-download)
sed 's/^\([[:space:]]*_checksum_style=\)"sha256"$/\1"md5"/' "$installer" >"$unknown/hennery-installer.sh"
refused unknown "$unknown" "$UNCHECKED"
echo "ok: an unknown checksum style aborts"
