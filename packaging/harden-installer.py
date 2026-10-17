#!/usr/bin/env python3
"""Make cargo-dist's shell installer refuse to install what it cannot check.

The distribution spec (§2) asks the installer to refuse to continue when no
SHA-256 tool is available, rather than skipping the check. cargo-dist's
installer (0.30.4) skips it, with a message, in four places: no `sha256sum`,
no checksum for the archive, an archive with no checksum style, and a style
it does not know. Each is replaced by an error. Where `sha256sum` is missing,
`shasum -a 256` (macOS's, and Perl's) is tried before refusing. Every
replaced text must occur exactly once: a cargo-dist upgrade that changes the
installer fails this script, and so the build, instead of shipping an
installer that skips the check again.

Usage: harden-installer.py <path to hennery-installer.sh>  (edited in place)
"""

import sys
from pathlib import Path

MAINTAINER_KEY = (Path(__file__).parent / 'release/maintainer.asc').read_text()
CONSUMER_FUNCTIONS = r'''release_git() {
    env GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0 GIT_NO_REPLACE_OBJECTS=1 \
        GNUPGHOME="$release_dir/keyring" git -c credential.helper= -c core.hooksPath=/dev/null "$@"
}
release_verify_tag() {
    case "$release_tag" in v*) ;; *) return 1 ;; esac
    printf '%s\n' "$release_tag" | LC_ALL=C grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' || return 1
    release_git init --bare --template= "$release_dir/source.git" >/dev/null 2>&1 || return 1
    release_git --git-dir="$release_dir/source.git" fetch --no-tags --depth=1 \
        https://github.com/marcinwadon/hennery.git "refs/tags/$release_tag" >/dev/null 2>&1 || return 1
    release_object=$(release_git --git-dir="$release_dir/source.git" rev-parse FETCH_HEAD) || return 1
    printf '%s\n' "$release_object" | LC_ALL=C grep -Eq '^[0-9a-f]{40}$' || return 1
    release_git --git-dir="$release_dir/source.git" cat-file tag "$release_object" > "$release_dir/tag-object" || return 1
    release_git --git-dir="$release_dir/source.git" -c gpg.format=openpgp -c gpg.program=gpg \
        verify-tag --raw "$release_object" > "$release_dir/tag-output" 2> "$release_dir/tag-status" || return 1
    awk -v fp="$release_fingerprint" '
        $1 == "[GNUPG:]" && $2 == "VALIDSIG" { count++; if ($3 != fp) bad=1 }
        $1 == "[GNUPG:]" && $2 ~ /^(BADSIG|ERRSIG|EXPSIG|EXPKEYSIG|REVKEYSIG|KEYEXPIRED|SIGEXPIRED|KEYREVOKED|NO_PUBKEY|NODATA|FAILURE|ERROR)$/ { bad=1 }
        END { exit !(count == 1 && !bad) }
    ' "$release_dir/tag-status" || return 1
    release_source=$(awk -v tag="$release_tag" '
        /^$/ { done=1 }
        !done && $1 == "object" { objects++; sha=$2; if (NF != 2 || length(sha) != 40 || sha ~ /[^0-9a-f]/) bad=1 }
        !done && $1 == "type" { types++; if ($0 != "type commit") bad=1 }
        !done && $1 == "tag" { tags++; if ($0 != "tag " tag) bad=1 }
        END { if (objects != 1 || types != 1 || tags != 1 || bad) exit 1; print sha }
    ' "$release_dir/tag-object") || return 1
}
release_hash() {
    if command -v sha256sum >/dev/null 2>&1; then
        release_actual=$(sha256sum -b "$1") || return 1
    elif command -v shasum >/dev/null 2>&1; then
        release_actual=$(shasum -a 256 -b "$1") || return 1
    else
        return 1
    fi
    release_digest=$(printf '%s\n' "$release_actual" | awk '{ print $1 }') || return 1
    printf '%s\n' "$release_digest" | LC_ALL=C grep -Eq '^[0-9a-f]{64}$' || return 1
}
release_attest() {
    release_bundle="${HENNERY_ATTESTATION_BUNDLE:-}"
    if [ -z "$release_bundle" ]; then
        release_hash "$1" || return 1
        release_bundle="$release_dir/sha256-$release_digest.jsonl"
        curl -q --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL \
            --url "https://api.github.com/users/marcinwadon/attestations/sha256:$release_digest?per_page=100&predicate_type=provenance" \
            -o "$release_dir/attestations.json" || return 1
        jq -er '.attestations | select(type == "array" and length > 0) | .[].bundle_url | select(type == "string")' \
            "$release_dir/attestations.json" > "$release_dir/bundle-urls" || return 1
        : > "$release_bundle" || return 1
        while IFS= read -r release_bundle_url; do
            curl -q --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL --url "$release_bundle_url" \
                -o "$release_dir/attestation.json" || return 1
            jq -c . "$release_dir/attestation.json" >> "$release_bundle" || return 1
        done < "$release_dir/bundle-urls"
    fi
    [ -s "$release_bundle" ] || return 1
    gh attestation verify "$1" --bundle "$release_bundle" --repo marcinwadon/hennery \
        --cert-identity "https://github.com/marcinwadon/hennery/.github/workflows/release.yml@refs/tags/$release_tag" \
        --source-ref "refs/tags/$release_tag" --source-digest "$release_source" --signer-digest "$release_source" \
        --cert-oidc-issuer https://token.actions.githubusercontent.com --predicate-type https://slsa.dev/provenance/v1 \
        --deny-self-hosted-runners >/dev/null || return 1
}
release_check_hash() {
    release_expected=$(awk -v name="$2" '$2 == name { count++; sha=$1 } END { if (count != 1) exit 1; print sha }' "$release_checksums") || return 1
    release_hash "$1" || return 1
    [ "$release_digest" = "$release_expected" ] || return 1
}
release_verify_payload() {
    [ -s "$release_checksums" ] || return 1
    release_attest "$release_checksums" || return 1
    release_last=$(tail -c 1 "$release_checksums") || return 1
    [ -z "$release_last" ] || return 1
    LC_ALL=C awk '
        { name=$2; lower=tolower(name) }
        length($1) != 64 || $1 ~ /[^0-9a-f]/ || name !~ /^[A-Za-z0-9][A-Za-z0-9._-]*$/ || $0 != $1 "  " name { bad=1 }
        lower == "sha256sums" || lower == "release.sigstore.jsonl" || seen[lower]++ || (NR > 1 && name <= previous) { bad=1 }
        { previous=name }
        END { exit !(NR > 0 && !bad) }
    ' "$release_checksums" || return 1
    release_check_hash "$1" "$2" || return 1
    release_attest "$1" || return 1
}
'''

MAINTAINER_VERIFICATION = r'''    # Verify signed source, the attested digest list and this archive before extraction.
    check_cmd git || err "git is required for signed release tag verification"
    check_cmd gpg || err "gpg is required for signed release tag verification"
    check_cmd curl || err "curl is required for public attestation lookup"
    check_cmd jq || err "jq is required for release provenance verification"
    check_cmd gh || err "GitHub CLI is required for release signature verification"
    release_dir=$(mktemp -d "${TMPDIR:-/tmp}/hennery-verify.XXXXXXXX") || err "cannot isolate release verification"
    release_tag="v$APP_VERSION"
    release_fingerprint=F26C7F594DFE84D131703AFC23E5A318AE8D2861
    mkdir -m 0700 "$release_dir/keyring" || err "cannot isolate release verification"
    printf 'no-autostart\nno-auto-key-retrieve\n' > "$release_dir/keyring/gpg.conf" || err "cannot isolate GPG options"
    cat > "$release_dir/keyring/key.asc" <<'HENNERY_MAINTAINER_KEY'
''' + MAINTAINER_KEY + r'''HENNERY_MAINTAINER_KEY
    gpg --batch --no-options --homedir "$release_dir/keyring" --no-autostart --no-auto-key-retrieve \
        --import "$release_dir/keyring/key.asc" >/dev/null 2>&1 || err "cannot import pinned maintainer public key"
    release_checksums="${HENNERY_RELEASE_CHECKSUMS:-$release_dir/SHA256SUMS}"
    if [ -z "${HENNERY_RELEASE_CHECKSUMS:-}" ]; then
        downloader "$ARTIFACT_DOWNLOAD_URL/SHA256SUMS" "$release_checksums" || err "cannot download release checksums"
    fi
    release_verify_tag || err "refusing an archive without a valid pinned signed release tag"
    release_verify_payload "$_file" "$_artifact_name" || err "refusing an archive without valid release-tag provenance and SHA256SUMS"

'''

REFUSE_UNCHECKED = (
    'err "refusing to install an archive whose checksum cannot be checked; '
    'install sha256sum or shasum and run the installer again"'
)

REPLACEMENTS = [
    (
        '    # ...and then the updater, if it exists',
        CONSUMER_FUNCTIONS + MAINTAINER_VERIFICATION + '    # ...and then the updater, if it exists',
    ),
    (
        '    case "$_checksum_style" in\n        sha256)',
        '    [ "$_checksum_style" = sha256 ] || ' + REFUSE_UNCHECKED + '\n'
        '    [ "${#_checksum_value}" = 64 ] || ' + REFUSE_UNCHECKED + '\n'
        '    case "$_checksum_value" in *[!0-9a-f]*) ' + REFUSE_UNCHECKED + ' ;; esac\n'
        '    case "$_checksum_style" in\n        sha256)',
    ),
    (
        '''        ensure mv "$_bin" "$_install_dir"
        # unzip seems to need this chmod
        ensure chmod +x "$_install_dir/$_bin_name"''',
        '''        # Refuse existing package-manager files, directories and dangling links.
        if [ -e "$_install_dir/$_bin_name" ] || [ -L "$_install_dir/$_bin_name" ]; then
            err "refusing to overwrite an existing installation; choose an empty user directory"
        fi
        local _stage
        _stage=$(mktemp -d "$_install_dir/.hennery-install.XXXXXX") || err "cannot stage installation"
        if ! cp "$_bin" "$_stage/$_bin_name" || ! chmod 0755 "$_stage/$_bin_name"; then
            rm -f "$_stage/$_bin_name"
            rmdir "$_stage"
            err "cannot prepare installation"
        fi
        # Target is the directory operand: link(2) atomically creates its
        # basename, refusing EEXIST, including a concurrently created link.
        if ! ln "$_stage/$_bin_name" "$_install_dir/"; then
            rm -f "$_stage/$_bin_name"
            rmdir "$_stage"
            err "refusing to overwrite an existing installation"
        fi
        rm -f "$_stage/$_bin_name"
        rmdir "$_stage"''',
    ),
    (
        # No `sha256sum`: `shasum`, else no SHA-256 tool at all.
        '''            if ! check_cmd sha256sum; then
                say "skipping sha256 checksum verification (it requires the 'sha256sum' command)"
                return 0
            fi
            _calculated_checksum="$(sha256sum -b "$_file" | awk '{printf $1}')"''',
        '''            if check_cmd sha256sum; then
                _calculated_checksum="$(sha256sum -b "$_file" | awk '{printf $1}')"
            elif check_cmd shasum; then
                _calculated_checksum="$(shasum -a 256 -b "$_file" | awk '{printf $1}')"
            else
                '''
        + REFUSE_UNCHECKED
        + '''
            fi''',
    ),
    (
        # A checksum style it does not know.
        '''        *)
            say "skipping unknown checksum style: $_checksum_style"
            return 0
            ;;''',
        '''        *)
            '''
        + REFUSE_UNCHECKED
        + '''
            ;;''',
    ),
    (
        # An empty checksum for the archive.
        '''    if [ -z "$_checksum_value" ]; then
        return 0
    fi''',
        '''    if [ -z "$_checksum_value" ]; then
        '''
        + REFUSE_UNCHECKED
        + '''
    fi''',
    ),
    (
        # An archive with no checksum at all.
        '''    else
        say "no checksums to verify"
    fi''',
        '''    else
        '''
        + REFUSE_UNCHECKED
        + '''
    fi''',
    ),
]


def harden(text: str) -> str:
    for old, new in REPLACEMENTS:
        count = text.count(old)
        if count != 1:
            raise SystemExit(
                f"harden-installer: expected one copy of this text, found {count}:\n{old}"
            )
        text = text.replace(old, new)
    return text


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    path = sys.argv[1]
    with open(path, encoding="utf-8") as f:
        text = f.read()
    # Before the file is opened for writing: a failure leaves it as it was.
    hardened = harden(text)
    with open(path, "w", encoding="utf-8") as f:
        f.write(hardened)


if __name__ == "__main__":
    main()
