#!/usr/bin/env python3
"""Assemble and materialize exact OCI bytes bound by release SHA256SUMS."""
import argparse
import io
import json
from pathlib import Path
import re
import sys
import tarfile

sys.path.insert(0, str(Path(__file__).parent))
sys.path.insert(0, str(Path(__file__).parent / 'release'))
import contract
import gate
import supply
import image

TARGETS = {'amd64': 'x86_64-unknown-linux-musl', 'arm64': 'aarch64-unknown-linux-musl'}


def descriptor(layout, binaries, revision, version):
    verified = image.verify(layout, binaries, revision, version)
    index = gate.strict_json((layout / 'index.json').read_bytes())
    gate.require(len(index['manifests']) == 1, 'expected one OCI index')
    return {'schema': 1, 'digest': index['manifests'][0]['digest'], 'source_sha': revision,
            'version': version, 'platforms': {a: p['digest'] for a, p in verified['platforms'].items()}}


def assemble(candidate, work, natives, revision, version):
    candidate, work = Path(candidate), Path(work)
    gate.require(not work.exists(), 'image assembly output already exists')
    work.mkdir(parents=True, exist_ok=True)
    binaries = supply.binary_inventory(candidate, sorted(TARGETS.values()))
    image.merge(work / 'layout', [Path(p) for p in natives])
    index = descriptor(work / 'layout', {a: binaries[t]['sha256'] for a, t in TARGETS.items()}, revision, version)
    with (candidate / 'ghcr-oci-layout.tar').open('xb') as output, tarfile.open(fileobj=output, mode='w') as archive:
        for path in sorted((work / 'layout').rglob('*')):
            if path.is_file():
                data = path.read_bytes()
                member = tarfile.TarInfo(path.relative_to(work / 'layout').as_posix())
                member.size = len(data); member.mode = 0o644
                archive.addfile(member, io.BytesIO(data))
    with (candidate / 'ghcr-index.json').open('xb') as output:
        output.write(gate.canonical(index))
    return index


def tar_files(raw):
    files = {}
    with tarfile.open(fileobj=io.BytesIO(raw), mode='r:') as archive:
        for member in archive:
            name = member.name
            gate.require(re.fullmatch(r'oci-layout|index\.json|blobs/sha256/[0-9a-f]{64}', name), 'unsafe OCI archive path')
            gate.require(member.isfile(), 'nonregular OCI archive member')
            gate.require(name not in files, 'duplicate OCI archive member')
            files[name] = archive.extractfile(member).read()
    return files


def retained_file(candidate, inventory, name):
    entry = next(a for a in inventory['artifacts'] if a['name'] == name)
    raw = (candidate / name).read_bytes()
    gate.require(len(raw) == entry['size'] and gate.digest(raw) == entry['sha256'], 'release OCI payload changed during verification')
    return raw


def materialize_layout(layout, files):
    if not layout.exists():
        layout.mkdir(parents=True)
        for name, raw in files.items():
            path = layout / name; path.parent.mkdir(parents=True, exist_ok=True)
            with path.open('xb') as output:
                output.write(raw)
    paths = list(layout.rglob('*'))
    gate.require(not layout.is_symlink() and all(not p.is_symlink() and (p.is_file() or p.is_dir()) for p in paths), 'nonregular working OCI')
    actual = {p.relative_to(layout).as_posix(): p.read_bytes() for p in paths if p.is_file()}
    gate.require(actual == files, 'working OCI differs from release layout')


def metadata(path, raw):
    if path.exists() or path.is_symlink():
        gate.require(path.is_file() and not path.is_symlink() and path.read_bytes() == raw, 'working image metadata changed')
    else:
        with path.open('xb') as output:
            output.write(raw)


def inspect_release(candidate, work, inventory):
    # Unprivileged data acceptance confers no publishing authority. Credentialed
    # callers authenticate the signed tag and tag/source attestations separately.
    gate.require(not work.resolve().is_relative_to(candidate.resolve()), 'OCI working directory must be outside immutable release')
    current = contract.load(candidate)
    gate.require(current == inventory, "release OCI metadata changed")
    gate.require('ghcr' in inventory['channels'] and set(TARGETS.values()) <= set(inventory['targets']), 'release needs GHCR and both Linux targets')
    supply.verify_binary_inventory(inventory, candidate)
    files = tar_files(retained_file(candidate, inventory, 'ghcr-oci-layout.tar'))
    index = gate.strict_json(retained_file(candidate, inventory, 'ghcr-index.json'))
    if work.exists():
        gate.require(work.is_dir() and not work.is_symlink(), 'nonregular image working directory')
    else:
        work.mkdir(parents=True)
    materialize_layout(work / 'layout', files)
    binaries = {a: inventory['binaries'][t]['sha256'] for a, t in TARGETS.items()}
    actual = descriptor(work / 'layout', binaries, inventory['source_sha'], inventory['tag'].removeprefix('v'))
    gate.require(index == actual, 'release image descriptor differs from actual OCI')
    metadata(work / 'verified.json', gate.canonical(actual))
    metadata(work / 'binary-sha256.json', gate.canonical(binaries))
    metadata(work / 'index-digest.txt', (actual['digest'] + '\n').encode())
    return actual


def inspect_release(candidate, work, inventory):
    # Data qualification is shared by the unprivileged aggregate and the
    # publishing verifier. Only qualify() authenticates publication authority.
    gate.require(not work.resolve().is_relative_to(candidate.resolve()), 'OCI working directory must be outside immutable candidate')
    import contract
    current = contract.load(candidate)
    gate.require(current == inventory, "release OCI metadata changed")
    gate.require('ghcr' in inventory['channels'] and set(TARGETS.values()) <= set(inventory['targets']), 'release needs GHCR and both Linux targets')
    supply.verify_binary_inventory(inventory, candidate)
    files = tar_files(retained_file(candidate, inventory, 'ghcr-oci-layout.tar'))
    index = gate.strict_json(retained_file(candidate, inventory, 'ghcr-index.json'))
    if work.exists():
        gate.require(work.is_dir() and not work.is_symlink(), 'nonregular image working directory')
    else:
        work.mkdir(parents=True)
    materialize_layout(work / 'layout', files)
    binaries = {a: inventory['binaries'][t]['sha256'] for a, t in TARGETS.items()}
    actual = descriptor(work / 'layout', binaries, inventory['source_sha'], inventory['tag'].removeprefix('v'))
    gate.require(index == actual, 'release image descriptor differs from actual OCI')
    metadata(work / 'verified.json', gate.canonical(actual))
    metadata(work / 'binary-sha256.json', gate.canonical(binaries))
    metadata(work / 'index-digest.txt', (actual['digest'] + '\n').encode())
    return actual


def accept(candidate, work, revision, version):
    candidate, work = Path(candidate), Path(work)
    inventory = contract.load(candidate)
    gate.require(inventory['source_sha'] == revision and inventory['tag'] == 'v' + version,
                 'release source/tag mismatch')
    return inspect_release(candidate, work, inventory)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    build = sub.add_parser('assemble')
    build.add_argument('candidate', type=Path); build.add_argument('work', type=Path)
    build.add_argument('revision'); build.add_argument('version')
    build.add_argument('natives', type=Path, nargs=2)
    acceptance = sub.add_parser('accept')
    acceptance.add_argument('candidate', type=Path); acceptance.add_argument('work', type=Path)
    acceptance.add_argument('revision'); acceptance.add_argument('version')
    args = parser.parse_args()
    if args.command == 'assemble':
        result = assemble(args.candidate, args.work, args.natives, args.revision, args.version)
    else:
        result = accept(args.candidate, args.work, args.revision, args.version)
    print(gate.canonical(result).decode(), end='')


if __name__ == '__main__':
    main()
