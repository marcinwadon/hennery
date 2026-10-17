#!/usr/bin/env python3
"""Use cargo-dist's real installer with tiny executable fixtures (no cross build).

This tests refusal and publication semantics, never native binary compatibility.
Native candidate installer jobs separately exercise the real release archives.
"""
import argparse
import io
from pathlib import Path
import re
import tarfile

import gate


def make(pristine, output):
    output.mkdir(parents=True, exist_ok=False)
    text = pristine.read_text()
    targets = [target for target in sorted(gate.TARGETS) if '"hennery-' + target + '.tar.xz")' in text]
    gate.require(targets, 'cargo-dist installer offers no qualified archive')
    for target in targets:
        name = 'hennery-' + target
        archive = output / (name + '.tar.xz')
        with tarfile.open(archive, 'w:xz') as tar:
            directory = tarfile.TarInfo(name + '/')
            directory.type = tarfile.DIRTYPE
            tar.addfile(directory)
            data = b'#!/bin/sh\necho "hennery installer-test fixture"\n'
            binary = tarfile.TarInfo(name + '/hennery')
            binary.mode, binary.size = 0o755, len(data)
            tar.addfile(binary, io.BytesIO(data))
            for document in ['CHANGELOG.md', 'LICENSE', 'README.md', 'THIRD-PARTY-NOTICES.txt']:
                data = (document + ' installer-test fixture\n').encode()
                entry = tarfile.TarInfo(name + '/' + document)
                entry.size = len(data)
                tar.addfile(entry, io.BytesIO(data))
        sha = gate.digest(archive.read_bytes())
        (output / (archive.name + '.sha256')).write_text(sha + '  ' + archive.name + '\n')
        anchor = '"' + archive.name + '")'
        gate.require(text.count(anchor) == 1, 'cargo-dist archive arm changed')
        start = text.index(anchor) + len(anchor)
        end = text.find(';;', start)
        gate.require(end != -1, 'cargo-dist archive arm has no terminator')
        # A native build already embeds checksums. Replace those assignments
        # within this arm so they cannot overwrite the tiny fixture's digest.
        arm = re.sub(r'(?m)^[ \t]*_checksum_(?:style|value)="[^"\n]*"\n', '', text[start:end])
        text = text[:start] + '\n            _checksum_style="sha256"\n            _checksum_value="' + sha + '"' + arm + text[end:]
    (output / 'hennery-installer.sh').write_text(text)
    # A retained candidate also carries application and dependency sources.
    # These are genuine tarballs, but the installer must never offer them.
    for name in ['hennery-source', *[target + '-third-party-source' for target in targets]]:
        with tarfile.open(output / (name + '.tar.xz'), 'w:xz') as tar:
            data = b'source fixture\n'
            entry = tarfile.TarInfo(name + '/README.md')
            entry.size = len(data)
            tar.addfile(entry, io.BytesIO(data))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('pristine', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    make(args.pristine, args.output)
