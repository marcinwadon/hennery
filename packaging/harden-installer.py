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

REFUSE_UNCHECKED = (
    'err "refusing to install an archive whose checksum cannot be checked; '
    'install sha256sum or shasum and run the installer again"'
)

REPLACEMENTS = [
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
