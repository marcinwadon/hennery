#!/bin/sh
# The hardened shell installer (distribution spec §2, §9), run against the
# archives of this build, served from a directory (`file://`):
#   - each executable archive is offered, with its SHA-256 embedded;
#     one corresponding-source archive is a separate release asset;
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

# Offline acceptance has no real release tag or attestation. These doubles
# enforce the exact public verifier invocation; consumer tests use real GPG tags.
fixture_version=$(sed -n 's/^APP_VERSION="\([^"]*\)"$/\1/p' "$installer")
mkdir "$scratch/verifier-tools"
printf 'offline verification fixture\n' > "$scratch/bundle.jsonl"
cat > "$scratch/verifier-tools/gh" <<TOOL
#!/bin/sh
set -eu
[ "\$1" = attestation ] && [ "\$2" = verify ] || exit 1
shift 3
identity=0
source=0
source_digest=0
signer_digest=0
while [ "\$#" -gt 0 ]; do
    case "\$1" in
        --cert-identity)
            [ "\$2" = "https://github.com/marcinwadon/hennery/.github/workflows/release.yml@\${HENNERY_TEST_SIGNER_REF:-refs/tags/v$fixture_version}" ] || exit 1
            identity=1; shift 2 ;;
        --source-ref)
            [ "\$2" = 'refs/tags/v$fixture_version' ] || exit 1
            source=1; shift 2 ;;
        --source-digest)
            [ "\$2" = 1111111111111111111111111111111111111111 ] || exit 1
            source_digest=1; shift 2 ;;
        --signer-digest)
            [ "\$2" = 1111111111111111111111111111111111111111 ] || exit 1
            signer_digest=1; shift 2 ;;
        --signer-workflow) exit 1 ;;
        *) shift ;;
    esac
done
[ "\$identity\$source\$source_digest\$signer_digest" = 1111 ] || exit 1
printf verified > '$scratch/signature-verified'
printf '%s\n' 'verified'
TOOL
chmod +x "$scratch/verifier-tools/gh"
cat > "$scratch/verifier-tools/gpg" <<'TOOL'
#!/bin/sh
for argument in "$@"; do [ "$argument" != --import ] || exit 0; done
exit 1
TOOL
cat > "$scratch/verifier-tools/git" <<TOOL
#!/bin/sh
set -eu
for argument in "\$@"; do
    case "\$argument" in
        init) exit 0 ;;
        fetch) exit 0 ;;
        rev-parse) echo 2222222222222222222222222222222222222222; exit 0 ;;
        cat-file)
            printf 'object 1111111111111111111111111111111111111111\ntype commit\ntag v$fixture_version\ntagger Disposable fixture\n\nOffline fixture\n'; exit 0 ;;
        verify-tag)
            case "\${HENNERY_TEST_TAG_SIGNATURE:-valid}" in
                unsigned|malformed) exit 1 ;;
                wrong-key) echo '[GNUPG:] VALIDSIG 0000000000000000000000000000000000000000' >&2; exit 0 ;;
            esac
            echo '[GNUPG:] VALIDSIG F26C7F594DFE84D131703AFC23E5A318AE8D2861' >&2; exit 0 ;;
    esac
done
exit 1
TOOL
chmod +x "$scratch/verifier-tools/gpg" "$scratch/verifier-tools/git"

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
    [ -f "$dist/$archive" ] || fail "the installer offers $archive, and this build has no $archive"
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
    [ -f "$archive" ] || continue
    case "${archive##*/}" in
        hennery-*-unknown-linux-musl.tar.xz | hennery-*-apple-darwin.tar.xz) ;;
        *) continue ;;
    esac
    printf '%s\n' "$arms" | grep -Fxq "${archive##*/}" ||
        fail "the installer does not offer ${archive##*/}"
    built=$((built + 1))
done
[ "$count" = "$built" ] || fail "the installer offers $count archives, this build made $built"
echo "ok: the installer embeds each of its $count archives' SHA-256"

# SHA256SUMS is independent of the installer's embedded checksum.
command -v jq >/dev/null || fail 'jq is required for provenance fixtures'
ln -s "$(command -v jq)" "$scratch/verifier-tools/jq"
: > "$scratch/SHA256SUMS"
for archive in $arms; do
    want=$(cut -d ' ' -f 1 "$dist/$archive.sha256")
    printf '%s  %s\n' "$want" "$archive" >> "$scratch/SHA256SUMS"
done
LC_ALL=C sort -k2 "$scratch/SHA256SUMS" > "$scratch/ordered-sums"
mv "$scratch/ordered-sums" "$scratch/SHA256SUMS"

# run_installer <download dir> <install dir> <log>: the installer's exit status.
run_installer() {
    mkdir -p "$scratch/home"
    status=0
    PATH="${VERIFIER_TOOLS:-$scratch/verifier-tools}:${INSTALLER_SYSTEM_PATH:-$PATH}" \
        HOME="$scratch/home" XDG_CONFIG_HOME="$scratch/home/.config" \
        HENNERY_ATTESTATION_BUNDLE="$scratch/bundle.jsonl" \
        HENNERY_RELEASE_CHECKSUMS="${TEST_CHECKSUMS:-$scratch/SHA256SUMS}" \
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
link_tool() {
    _link_source=$1
    _link_destination=$2
    if [ -e "$_link_destination" ] || [ -L "$_link_destination" ]; then
        [ -L "$_link_destination" ] && [ "$_link_destination" -ef "$_link_source" ] ||
            fail "conflicting tool link: $_link_destination"
    else
        ln -s "$_link_source" "$_link_destination" || fail "cannot link tool: $_link_destination"
    fi
}

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
            link_tool "$tool" "$dir/$name"
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
[ -f "$scratch/signature-verified" ] || fail 'installer never authenticated release provenance'
echo "ok: installs a binary that runs"

# With `shasum` and no `sha256sum`.
tools "$scratch/shasum-tools" sha256sum
# In a subshell: an assignment before a function call may outlive it.
(INSTALLER_SYSTEM_PATH="$scratch/shasum-tools" run_installer "$dist" "$scratch/shasum" "$scratch/shasum.log") || {
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
(INSTALLER_SYSTEM_PATH="$scratch/no-tools" refused unchecked "$dist" "$UNCHECKED")
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

# Existing installs are explicit operator choices. Never replace any of them.
mkdir -p "$scratch/existing" "$scratch/link" "$scratch/dangling" "$scratch/directory/hennery"
printf 'owned by another package manager\n' > "$scratch/existing/hennery"
cp "$scratch/existing/hennery" "$scratch/sentinel"
ln -s "$scratch/sentinel" "$scratch/link/hennery"
ln -s "$scratch/absent-target" "$scratch/dangling/hennery"
for kind in existing link dangling directory; do
    if run_installer "$dist" "$scratch/$kind" "$scratch/$kind.log"; then
        fail "$kind: replaced an existing installation"
    fi
    grep -q 'refusing to overwrite' "$scratch/$kind.log" || fail "$kind: wrong refusal"
done
cmp "$scratch/existing/hennery" "$scratch/sentinel" || fail 'existing install changed'
[ -L "$scratch/link/hennery" ] || fail 'package-manager symlink changed'
[ -L "$scratch/dangling/hennery" ] || fail 'dangling symlink changed'
[ -d "$scratch/directory/hennery" ] || fail 'directory changed'
echo 'ok: existing files, symlinks and directories survive'

# Missing download: no destination is published.
missing=$(copy missing-download)
for archive in "$missing"/*.tar.xz; do
    rm "$archive"
done
if run_installer "$missing" "$scratch/missing" "$scratch/missing.log"; then
    fail 'missing download installed'
fi
[ ! -e "$scratch/missing/hennery" ] || fail 'missing download left a binary'
echo 'ok: failed download leaves no binary'

# Known non-SHA256 algorithms and malformed values must not enter skip paths.
sha512=$(copy sha512-download)
sed 's/^\([[:space:]]*_checksum_style=\)"sha256"$/\1"sha512"/' "$installer" > "$sha512/hennery-installer.sh"
refused sha512 "$sha512" "$UNCHECKED"
malformed=$(copy malformed-download)
sed 's/^\([[:space:]]*_checksum_value=\)".*"$/\1"not-a-digest"/' "$installer" > "$malformed/hennery-installer.sh"
refused malformed "$malformed" "$UNCHECKED"
echo 'ok: non-SHA256 and malformed digest abort'

# Copy and chmod fail before the executable becomes visible. The installer
# uses a private sibling directory, then an exclusive hard link into install dir.
for command in cp chmod ln; do
    tool_dir="$scratch/failing-$command-tools"
    tools "$tool_dir" "$command"
    cat > "$tool_dir/$command" <<'TOOL'
#!/bin/sh
exit 1
TOOL
    chmod +x "$tool_dir/$command"
    (! INSTALLER_SYSTEM_PATH="$tool_dir" run_installer "$dist" "$scratch/failing-$command" "$scratch/failing-$command.log") ||
        fail "$command failure installed"
    [ ! -e "$scratch/failing-$command/hennery" ] || fail "$command failure published an executable"
done
echo 'ok: copy, chmod and publication failures leave no executable'

# Unsupported OS and CPU each refuse before installation.
real_uname=$(command -v uname)
for kind in os cpu; do
    tool_dir="$scratch/unsupported-$kind-tools"
    tools "$tool_dir" uname
    cat > "$tool_dir/uname" <<TOOL
#!/bin/sh
if [ '$kind' = os ]; then echo UnsupportedOS; exit 0; fi
if [ "\${1:-}" = -m ]; then echo unsupported-cpu; exit 0; fi
exec '$real_uname' "\$@"
TOOL
    chmod +x "$tool_dir/uname"
    (! INSTALLER_SYSTEM_PATH="$tool_dir" run_installer "$dist" "$scratch/unsupported-$kind" "$scratch/unsupported-$kind.log") ||
        fail "unsupported $kind installed"
    [ ! -e "$scratch/unsupported-$kind/hennery" ] || fail "unsupported $kind left a binary"
done
echo 'ok: unsupported OS and CPU abort'

# The copy runs while the final pathname is absent, and a destination raced
# into existence after the initial check is never replaced.
real_cp=$(command -v cp)
real_ln=$(command -v ln)
tools "$scratch/observe-copy-tools" cp
cat > "$scratch/observe-copy-tools/cp" <<TOOL
#!/bin/sh
[ ! -e "\$HENNERY_UNMANAGED_INSTALL/hennery" ] || exit 1
printf observed > '$scratch/copy-observed'
exec '$real_cp' "\$@"
TOOL
chmod +x "$scratch/observe-copy-tools/cp"
(INSTALLER_SYSTEM_PATH="$scratch/observe-copy-tools" run_installer "$dist" "$scratch/observe-copy" "$scratch/observe-copy.log") || fail 'staged copy failed'
[ -f "$scratch/copy-observed" ] || fail 'the installer did not stage the copy'
tools "$scratch/raced-tools" ln
cat > "$scratch/raced-tools/ln" <<TOOL
#!/bin/sh
printf raced > "\$HENNERY_UNMANAGED_INSTALL/hennery"
exec '$real_ln' "\$@"
TOOL
chmod +x "$scratch/raced-tools/ln"
(! INSTALLER_SYSTEM_PATH="$scratch/raced-tools" run_installer "$dist" "$scratch/raced" "$scratch/raced.log") || fail 'concurrent destination was replaced'
[ "$(cat "$scratch/raced/hennery")" = raced ] || fail 'concurrent destination changed'
echo 'ok: staging is invisible and concurrent destinations survive'

# Signed source and attested checksums are mandatory.
for kind in unsigned malformed wrong-key; do
    (export HENNERY_TEST_TAG_SIGNATURE="$kind"; refused "tag-$kind" "$dist" 'valid pinned signed release tag')
done
echo 'ok: unsigned tag, malformed signature and another signing key abort'
(TEST_CHECKSUMS="$scratch/missing-sums"; export TEST_CHECKSUMS; refused missing-sums "$dist" 'release-tag provenance')
echo 'ok: missing SHA256SUMS aborts'
sed 's/[0-9a-f]/0/g' "$scratch/SHA256SUMS" > "$scratch/wrong-sums"
(TEST_CHECKSUMS="$scratch/wrong-sums"; export TEST_CHECKSUMS; refused other-digest "$dist" 'release-tag provenance')
echo 'ok: SHA256SUMS must authorize the exact archive bytes'
(export HENNERY_TEST_SIGNER_REF=refs/heads/main; refused branch-signer "$dist" 'refusing an archive without valid release-tag provenance')
echo 'ok: branch signer identity cannot authorize installation'
tools "$scratch/no-gh-tools" gh gpg jq git
link_tool "$scratch/verifier-tools/gpg" "$scratch/no-gh-tools/gpg"
link_tool "$scratch/verifier-tools/jq" "$scratch/no-gh-tools/jq"
link_tool "$scratch/verifier-tools/git" "$scratch/no-gh-tools/git"
(INSTALLER_SYSTEM_PATH="$scratch/no-gh-tools" VERIFIER_TOOLS="$scratch/no-gh-tools" refused missing-verifier "$dist" 'GitHub CLI is required')
echo 'ok: missing signature verifier refuses installation'
