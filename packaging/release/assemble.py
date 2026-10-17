#!/usr/bin/env python3
"""Merge native material without last-writer-wins artifact replacement."""
import argparse
from pathlib import Path
import shutil
import tarfile

import gate
import contract


def native_files(target):
    base = 'hennery-' + target
    return {base + '.tar.xz', base + '.tar.xz.sha256', base + '.spdx.json',
            target + '-dist-manifest.json', 'THIRD-PARTY-NOTICES.txt', 'licence-findings.json',
            'builder.json', 'third-party-source.tar.xz', 'rust-advisories.json',
            'rust-advisory-revision.txt', 'web-advisories.json', 'build-identity.json',
            'runtime-inventory.json'}


def check_native(directory, target):
    """Pinned dist leaves its expanded archive in distrib; it is not a payload.

    upload-artifact preserves those descendants, and pattern downloads retain
    the candidate-* wrapper. Validate the exact tree before projecting files;
    flattening downloads would erase conflicting shared material.
    """
    gate.require(directory.is_dir() and not directory.is_symlink(), 'nonregular native candidate')
    base = 'hennery-' + target
    gate.require({p.name for p in directory.iterdir()} == native_files(target) | {base},
                 'unexpected or missing native material: ' + target)
    for name in native_files(target):
        contract.regular(directory / name)
    staging = directory / base
    gate.require(staging.is_dir() and not staging.is_symlink(), 'nonregular native staging directory')
    expected = {'hennery', 'CHANGELOG.md', 'LICENSE', 'README.md'}
    gate.require({p.name for p in staging.iterdir()} == expected, 'unexpected native staging contents')
    with tarfile.open(directory / (base + '.tar.xz'), 'r:xz') as archive:
        members = archive.getmembers()
        for name in sorted(expected):
            matching = [m for m in members if m.name == base + '/' + name]
            gate.require(len(matching) == 1 and matching[0].isfile(), 'nonregular native archive member')
            gate.require(contract.regular(staging / name) == archive.extractfile(matching[0]).read(),
                         'native staging differs from archive')


def assemble(candidates, output, targets=None):
    targets = sorted(gate.TARGETS) if targets is None else targets
    gate.require(targets and set(targets) <= gate.TARGETS, "unqualified assembly targets")
    output = Path(output)
    output.mkdir(parents=True, exist_ok=False)
    notices, findings = [], []
    candidates = Path(candidates)
    gate.require(candidates.is_dir() and not candidates.is_symlink(), 'nonregular candidates directory')
    expected = {'candidate-' + t for t in targets}
    gate.require({p.name for p in Path(candidates).iterdir()} == expected, 'native candidate target set incomplete')
    for directory in sorted(Path(candidates).iterdir()):
        target = directory.name.removeprefix('candidate-')
        check_native(directory, target)
        for path in sorted(directory / name for name in native_files(target)):
            name = path.name
            gate.require(path.is_file() and not path.is_symlink(), 'nonregular native material')
            if name == 'THIRD-PARTY-NOTICES.txt':
                notices.append(f'\n===== {target} =====\n' + path.read_text())
                continue
            if name == 'licence-findings.json':
                findings.extend(gate.strict_json(path.read_bytes())['findings'])
                continue
            if name in ('builder.json', 'third-party-source.tar.xz', 'rust-advisories.json', 'rust-advisory-revision.txt', 'web-advisories.json'):
                name = target + '-' + name
            destination = output / name
            if destination.exists():
                gate.require(destination.read_bytes() == path.read_bytes(), f'conflicting candidate material: {name}')
            else:
                shutil.copyfile(path, destination)
    (output / 'THIRD-PARTY-NOTICES.txt').write_text(''.join(notices))
    (output / 'licence-findings.json').write_bytes(gate.canonical({'findings': sorted(set(findings))}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('candidates', type=Path)
    parser.add_argument('output', type=Path, nargs='?')
    parser.add_argument('--targets', default=','.join(sorted(gate.TARGETS)))
    parser.add_argument('--check-native', choices=sorted(gate.TARGETS))
    args = parser.parse_args()
    if args.check_native:
        check_native(args.candidates, args.check_native)
    else:
        if args.output is None:
            parser.error('output is required for assembly')
        assemble(args.candidates, args.output, args.targets.split(','))
