"""Seven public assets; qualification and attestation inputs stay in Actions."""
import io
from pathlib import Path
import subprocess
import tarfile

import contract
import gate
import supply

PUBLIC_CHECKSUMS = 'public-SHA256SUMS'
SBOM = 'hennery.spdx.json'
SOURCE = 'hennery-source.tar.xz'


def names(targets):
    gate.require(targets and set(targets) <= gate.TARGETS, 'unqualified public targets')
    return sorted(['hennery-installer.sh', SOURCE, SBOM,
                   *['hennery-' + target + '.tar.xz' for target in targets]])


def prepare(root, source, targets):
    root = Path(root)
    # Preserve complete corresponding source (including the linked MPL WebAuthn
    # crates) once. Identical registry sources from native builders are deduped;
    # differing bytes under one package path stop assembly.
    seen = {}
    with tarfile.open(root / SOURCE, 'w:xz', preset=1) as output:
        def add(member, data):
            gate.require(member.isfile() and not Path(member.name).is_absolute()
                         and '..' not in Path(member.name).parts, 'unsafe corresponding source member')
            digest = gate.digest(data)
            if member.name in seen:
                gate.require(seen[member.name] == digest, 'conflicting corresponding source: ' + member.name)
            else:
                seen[member.name] = digest
                output.addfile(member, io.BytesIO(data))
        raw = subprocess.check_output(['git', 'archive', '--format=tar', '--prefix=hennery-source/', source])
        with tarfile.open(fileobj=io.BytesIO(raw)) as own:
            for member in own:
                if not member.isdir():
                    add(member, own.extractfile(member).read() if member.isfile() else b'')
        for target in targets:
            with tarfile.open(root / (target + '-third-party-source.tar.xz'), 'r:xz') as dependencies:
                for member in dependencies:
                    if member.isdir():
                        continue
                    member.name = 'hennery-source/third-party/' + member.name
                    add(member, dependencies.extractfile(member).read() if member.isfile() else b'')
    packages = {}
    for target in targets:
        document = gate.strict_json(contract.regular(root / ('hennery-' + target + '.spdx.json')))
        for package in document['packages']:
            key = package['SPDXID']
            gate.require(key not in packages or packages[key] == package, 'conflicting combined SBOM component')
            packages[key] = package
    (root / SBOM).write_bytes(gate.canonical(supply.spdx([packages[key] for key in sorted(packages)], source, 'all-targets')))
    raw = ''.join(gate.digest(contract.regular(root / name)) + '  ' + name + '\n' for name in names(targets)).encode('ascii')
    contract.parse_checksums(raw)
    (root / PUBLIC_CHECKSUMS).write_bytes(raw)


def files(root, targets):
    root = Path(root)
    expected = names(targets)
    checksums = contract.parse_checksums(contract.regular(root / PUBLIC_CHECKSUMS))
    gate.require(checksums == {name: gate.digest(contract.regular(root / name)) for name in expected},
                 'public checksum inventory differs from frozen assets')
    return {contract.CHECKSUMS: (root / PUBLIC_CHECKSUMS).absolute(),
            **{name: (root / name).absolute() for name in expected}}


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=Path)
    parser.add_argument('source')
    parser.add_argument('--targets', required=True)
    args = parser.parse_args()
    prepare(args.root, args.source, args.targets.split(','))
