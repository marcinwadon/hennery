#!/usr/bin/env python3
"""Generate notices, source material, SPDX and final release materials before attestation.

Includes build/dev dependencies conservatively. Missing licence material is a
finding, never automatically treated as permission to redistribute. Downloaded
host runtimes are described separately and never included as payloads.
"""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import subprocess
import tarfile
import uuid
import urllib.parse

import gate
import contract
from adapter_manifest import read_manifest


def tree_digest(files):
    return gate.digest(gate.canonical([{'name': name, 'sha256': gate.digest(data)} for name, data in sorted(files.items())]))


def component(name, version, licence, root, ecosystem):
    notices = []
    for path in sorted(root.rglob('*')):
        if path.is_file() and not path.is_symlink() and not any(p in ('node_modules', '.git', 'target') for p in path.relative_to(root).parts[:-1]):
            if path.name.lower().startswith(('license', 'licence', 'copying', 'notice', 'copyright')) or any(part.lower() in ('licenses', 'licences') for part in path.relative_to(root).parts[:-1]):
                notices.append((str(path.relative_to(root)), path.read_text(errors='replace')))
    identifier = 'SPDXRef-' + gate.digest(f'{ecosystem}:{name}@{version}'.encode())[:24]
    return {'SPDXID': identifier, 'name': name, 'versionInfo': version, 'downloadLocation': 'NOASSERTION',
            'filesAnalyzed': False, 'licenseConcluded': 'NOASSERTION', 'licenseDeclared': ('MIT OR Apache-2.0' if licence == 'MIT/Apache-2.0' else licence) or 'NOASSERTION',
            'copyrightText': 'NOASSERTION', 'externalRefs': [{'referenceCategory': 'PACKAGE-MANAGER', 'referenceType': 'purl',
                                                            'referenceLocator': f'pkg:{ecosystem}/{urllib.parse.quote(name, safe="/")}@{urllib.parse.quote(version, safe="")}'}]}, notices



def frontend_sources(frontend):
    # pnpm 12 groups by licence and coalesces package versions/paths. Read
    # each package manifest so a coalesced entry never pairs the wrong version.
    for licence, group in frontend.items():
        for p in group:
            for raw_path in p['paths']:
                root = Path(raw_path)
                manifest = gate.strict_json((root / 'package.json').read_bytes())
                gate.require(manifest['name'] == p['name'] and manifest['version'] in p['versions'], 'frontend licence input differs from package bytes')
                yield p['name'], manifest['version'], licence, root, 'npm'



def rust_packages(cargo):
    if not cargo.get('resolve'):
        return cargo['packages']
    nodes = {node['id']: node for node in cargo['resolve']['nodes']}
    reachable, todo = set(), list(cargo['workspace_members'])
    while todo:
        node = todo.pop()
        if node not in reachable:
            reachable.add(node)
            todo.extend(nodes[node]['dependencies'])
    return [p for p in cargo['packages'] if p['id'] in reachable]


def corresponding_packages(cargo):
    """The shipped CLI's normal/build closure; unrelated workspace/dev tools are excluded."""
    if not cargo.get('resolve'):
        return cargo['packages']
    nodes = {node['id']: node for node in cargo['resolve']['nodes']}
    roots = [p['id'] for p in cargo['packages'] if p['name'] == 'hennery' and p['id'] in cargo['workspace_members']]
    gate.require(len(roots) == 1, 'corresponding source requires the shipped CLI root')
    reachable, todo = set(), roots
    while todo:
        identifier = todo.pop()
        if identifier not in reachable:
            reachable.add(identifier)
            todo.extend(dep['pkg'] for dep in nodes[identifier]['deps']
                        if any(kind['kind'] in (None, 'build') for kind in dep['dep_kinds']))
    return [p for p in cargo['packages'] if p['id'] in reachable]


def supplemental(ecosystem, name, version):
    root = Path(__file__).resolve().parent / 'licences'
    path = root / 'manifest.json'
    if not path.is_file():
        return []
    manifest = gate.strict_json(path.read_bytes())
    texts = []
    for entry in manifest.get(f'{ecosystem}:{name}@{version}', []):
        text = (root / entry['file']).read_bytes()
        gate.require(gate.digest(text) == entry['sha256'], 'supplemental licence text changed')
        texts.append((entry['source'], text.decode()))
    return texts


def components(cargo, frontend):
    packages, notices, findings = [], [], []
    sources = []
    for p in rust_packages(cargo):
        if p.get('source') is not None:
            sources.append((p['name'], p['version'], p.get('license'), Path(p['manifest_path']).parent, 'cargo'))
    sources.extend(frontend_sources(frontend))
    seen = set()
    for name, version, licence, root, ecosystem in sorted(sources):
        if (ecosystem, name, version) in seen:
            continue
        seen.add((ecosystem, name, version))
        p, texts = component(name, version, licence, root, ecosystem)
        packages.append(p)
        if not texts:
            texts = supplemental(ecosystem, name, version)
        if not licence or licence in ('UNKNOWN', 'UNLICENSED', 'NOASSERTION') or not texts:
            findings.append(f'{ecosystem}:{name}@{version}: missing licence expression or text')
        notices.append(f'\n===== {ecosystem}:{name}@{version} ({licence}) =====\n')
        for path, text in texts:
            notices.append(f'\n--- {path} ---\n{text}\n')
    # Name the native libraries separately, rather than implying their Rust
    # wrapper's version is the bundled C library version.
    for native in rust_packages(cargo):
        root = Path(native['manifest_path']).parent
        if native['name'] == 'libsqlite3-sys':
            header = root / 'sqlite3/sqlite3.h'
            if header.is_file():
                version = re.search(r'#define SQLITE_VERSION\s+"([^"]+)"', header.read_text())
                gate.require(version is not None, 'bundled SQLite version missing')
                p, _ = component('SQLite', version.group(1), None, root / 'sqlite3', 'generic')
                p['sourceInfo'] = 'Bundled SQLite C source in libsqlite3-sys; public-domain declaration preserved in sqlite3.c and corresponding source.'
                packages.append(p)
                notices.append('\n===== Bundled SQLite public-domain declaration =====\n' + (root / 'sqlite3/sqlite3.c').read_text()[:1000])
        if native['name'] == 'openssl-src':
            version_file = root / 'openssl/VERSION.dat'
            if version_file.is_file():
                fields = dict(re.findall(r'^(MAJOR|MINOR|PATCH)=(\d+)$', version_file.read_text(), re.M))
                gate.require(set(fields) == {'MAJOR', 'MINOR', 'PATCH'}, 'bundled OpenSSL version missing')
                p, _ = component('OpenSSL', '.'.join(fields[k] for k in ('MAJOR', 'MINOR', 'PATCH')), 'Apache-2.0', root / 'openssl', 'generic')
                p['sourceInfo'] = 'Vendored OpenSSL source shipped by openssl-src; exact source preserved.'
                packages.append(p)
    return packages, ''.join(notices), findings


def spdx(packages, source_sha, target):
    return {'spdxVersion': 'SPDX-2.3', 'dataLicense': 'CC0-1.0', 'SPDXID': 'SPDXRef-DOCUMENT',
            'name': f'hennery-{target}', 'documentNamespace': f'https://github.com/{gate.REPOSITORY}/releases/spdx/{source_sha}/{target}/{uuid.uuid4()}',
            'creationInfo': {'created': datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ'), 'creators': ['Tool: hennery-release-r1']},
            'packages': packages, 'relationships': [{'spdxElementId': 'SPDXRef-DOCUMENT', 'relationshipType': 'DESCRIBES',
                                                   'relatedSpdxElement': p['SPDXID']} for p in packages]}


def checksums(root):
    root = Path(root)
    entries = [(p.name, gate.digest(contract.regular(p))) for p in sorted(root.iterdir()) if p.name != contract.CHECKSUMS]
    raw = ''.join(f'{sha}  {name}\n' for name, sha in entries).encode('ascii')
    contract.parse_checksums(raw)
    (root / contract.CHECKSUMS).write_bytes(raw)
    verify_checksums(root)


def verify_checksums(root):
    return contract.verify_files(root)


def freeze(root, metadata):
    # Data generation only; the tag workflow supplies signature-verified source
    # metadata. Public consumers must still verify tag and every attestation.
    root = Path(root)
    gate.require(not any((root / name).exists() or (root / name).is_symlink() for name in (contract.METADATA, contract.CHECKSUMS)), 'release already frozen')
    gate.require(not gate.strict_json(contract.regular(root / 'licence-findings.json'))['findings'], 'unresolved licence findings: maintainer review/materials required')
    contract.validate_metadata(metadata)
    require_contract(root, metadata['channels'], metadata['targets'], metadata['tag'])
    metadata = metadata | {'binaries': binary_inventory(root, metadata['targets'])}
    return contract.freeze(root, metadata)


def dependency_sources(cargo, frontend, destination, prepared_frontend=None):
    # Include registry source, native vendored source (OpenSSL, SQLite), web
    # packages and licence texts. No managed Node or agent downloads.
    roots = [('cargo', p['name'], p['version'], Path(p['manifest_path']).parent) for p in corresponding_packages(cargo) if p.get('source')]
    roots += [(ecosystem, name, version, root) for name, version, _, root, ecosystem in frontend_sources(frontend)]
    seen = set()
    # Materialize filesystem hardlinks as regular bytes for the later merge.
    # Filesystem symlinks are still refused below before archive.add().
    with tarfile.open(destination, 'w:xz', dereference=True, preset=1) as archive:
        for ecosystem, name, version, root in sorted(roots):
            prefix = f'{ecosystem}/{name.replace("/", "_")}@{version}'
            if prefix in seen:
                continue
            seen.add(prefix)
            for path in sorted(root.rglob('*')):
                relative = path.relative_to(root)
                if any(part in ('node_modules', '.git', 'target') for part in relative.parts):
                    continue
                gate.require(not path.is_symlink(), f'third-party source symlink needs review: {prefix}/{relative}')
                if path.is_file():
                    archive.add(path, arcname=f'{prefix}/{relative}', recursive=False)
        if prepared_frontend is not None:
            with tarfile.open(prepared_frontend / 'third-party-source.tar.xz', 'r:xz') as frontend_archive:
                for member in frontend_archive:
                    gate.require(member.isfile() and member.name.startswith('npm/')
                                 and '..' not in Path(member.name).parts, 'invalid prepared frontend source')
                    archive.addfile(member, frontend_archive.extractfile(member))




def binary_inventory(root, targets):
    binaries = {}
    for target in targets:
        base = 'hennery-' + target
        archive = Path(root) / (base + '.tar.xz')
        with tarfile.open(archive, 'r:xz') as tar:
            members = tar.getmembers()
            expected = {base, base + '/hennery', base + '/CHANGELOG.md', base + '/LICENSE', base + '/README.md', base + '/THIRD-PARTY-NOTICES.txt'}
            gate.require(len(members) == len(expected) and {m.name.rstrip('/') for m in members} == expected,
                         'release archive content inventory differs')
            gate.require(all(m.isdir() if m.name.rstrip('/') == base else m.isfile() for m in members), 'release archive contains nonregular payload')
            data = tar.extractfile(base + '/hennery').read()
        sidecar = (Path(root) / (archive.name + '.sha256')).read_text().split()
        gate.require(sidecar == [gate.digest(archive.read_bytes()), archive.name], 'per-archive checksum sidecar differs')
        binaries[target] = {'sha256': gate.digest(data), 'size': len(data), 'archive': archive.name, 'member': base + '/hennery'}
    return binaries


def verify_binary_inventory(inventory, root):
    gate.require(inventory.get('binaries') == binary_inventory(root, inventory['targets']), 'release binary metadata differs from archive bytes')


def required_payloads(channels, targets, tag):
    required = {'hennery-installer.sh', 'hennery.rb', 'hennery-source.tar.xz', 'dist-manifest.json',
                'THIRD-PARTY-NOTICES.txt', 'licence-findings.json', 'runtime-inventory.json', 'build-identity.json', 'release-notes.md'}
    for target in targets:
        base = 'hennery-' + target
        required.update({base + '.tar.xz', base + '.tar.xz.sha256', base + '.spdx.json',
                         target + '-third-party-source.tar.xz', target + '-builder.json', target + '-rust-advisories.json',
                         target + '-rust-advisory-revision.txt', target + '-web-advisories.json'})
    if 'crates' in channels:
        required.update(name + '-' + tag.removeprefix('v') + '.crate' for name in
                        ('hennery-proto', 'hennery-kernel', 'hennery-view', 'hennery-host', 'hennery-gateway', 'hennery-sessions', 'hennery'))
    if 'ghcr' in channels:
        required.update({'ghcr-index.json', 'ghcr-oci-layout.tar'})
    return required


def require_contract(root, channels, targets, tag):
    gate.require(set(channels) <= gate.CHANNELS and set(targets) <= gate.TARGETS, 'unqualified release channel/target')
    gate.require('-' not in tag or channels == ['github'], 'prerelease channels are GitHub only')
    gate.require(required_payloads(channels, targets, tag) <= {p.name for p in Path(root).iterdir()}, 'required release/channel payload missing')
    if 'homebrew' in channels:
        import homebrew
        homebrew.check(root, tag, targets)
    if 'crates' in channels:
        import tomllib
        import verify
        base = 'hennery-' + tag.removeprefix('v')
        with tarfile.open(Path(root) / (base + '.crate'), 'r:gz') as archive:
            entries = [m for m in archive.getmembers() if m.name == base + '/Cargo.toml']
            gate.require(len(entries) == 1 and entries[0].isfile() and entries[0].size <= 1024*1024, 'crate metadata must be one bounded regular file')
            with archive.extractfile(entries[0]) as metadata:
                package = tomllib.loads(metadata.read().decode())['package']
        verify.require_binstall_metadata(package.get('metadata', {}), tag)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['frontend', 'materials'])
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--source-sha')
    parser.add_argument('--target')
    parser.add_argument('--cargo-metadata', type=Path)
    parser.add_argument('--frontend-licences', type=Path)
    parser.add_argument('--frontend-materials', type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.operation == 'frontend':
        gate.require(args.frontend_licences is not None and args.frontend_materials is None,
                     'frontend preparation requires local licences')
        frontend = gate.strict_json(args.frontend_licences.read_bytes())
        packages, notices, findings = components({'packages': []}, frontend)
        (args.output / 'frontend.json').write_bytes(gate.canonical({
            'packages': packages, 'notices': notices, 'findings': findings}))
        dependency_sources({'packages': []}, frontend, args.output / 'third-party-source.tar.xz')
    if args.operation == 'materials':
        gate.require(args.source_sha is not None, 'materials require source SHA')
        gate.require((args.frontend_licences is None) != (args.frontend_materials is None),
                     'choose prepared frontend materials or local licences')
        cargo = gate.strict_json(args.cargo_metadata.read_bytes())
        frontend = gate.strict_json(args.frontend_licences.read_bytes()) if args.frontend_licences else {}
        packages, notices, findings = components(cargo, frontend)
        if args.frontend_materials is not None:
            prepared = gate.strict_json((args.frontend_materials / 'frontend.json').read_bytes())
            packages += prepared['packages']; notices += prepared['notices']; findings += prepared['findings']
        (args.output / 'THIRD-PARTY-NOTICES.txt').write_text(notices)
        (args.output / f'hennery-{args.target}.spdx.json').write_bytes(gate.canonical(spdx(packages, args.source_sha, args.target)))
        (args.output / 'licence-findings.json').write_bytes(gate.canonical({'findings': findings, 'scope': 'Rust and frontend including build/dev; assets and corresponding source still require maintainer review'}))
        dependency_sources(cargo, frontend, args.output / 'third-party-source.tar.xz', args.frontend_materials)
        manifest = read_manifest()
        (args.output / 'runtime-inventory.json').write_bytes(gate.canonical({'adapter_manifest_sha256': gate.digest(manifest),
                                                                          'redistributed': False, 'manifest': gate.strict_json(manifest)}))
        ui = {str(p.relative_to('web/dist')): p.read_bytes() for p in Path('web/dist').rglob('*') if p.is_file()}
        gate.require('index.html' in ui, 'release UI missing')
        identity = {'source_sha': args.source_sha, 'ui_sha256': tree_digest(ui), 'adapter_manifest_sha256': gate.digest(manifest),
                    'cargo_lock_sha256': gate.digest(Path('Cargo.lock').read_bytes()), 'web_lock_sha256': gate.digest(Path('web/pnpm-lock.yaml').read_bytes())}
        (args.output / 'build-identity.json').write_bytes(gate.canonical(identity))


if __name__ == '__main__':
    try:
        main()
    except (gate.Refused, KeyError, OSError, ValueError) as error:
        raise SystemExit(f'release materials refused: {error}') from error
