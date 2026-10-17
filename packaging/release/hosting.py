"""Standard gh release hosting of already qualified files; never builds or clobbers."""
import os
from pathlib import Path
import re
import subprocess
import tempfile

import contract
import gate
import verifier_process


def command(*arguments):
    # Hosting needs only its step's GitHub token, never channel/OIDC credentials
    # or the operator's stored CLI configuration. Verification remains tokenless.
    with verifier_process.environment() as env:
        env.update(GH_TOKEN=os.environ['GH_TOKEN'], GH_PROMPT_DISABLED='1',
                   GH_NO_UPDATE_NOTIFIER='1', GH_TELEMETRY='false')
        try:
            return subprocess.run(['gh', 'release', *map(str, arguments), '--repo', gate.REPOSITORY],
                                  env=env, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired as error:
            raise gate.Refused('gh release timed out; reconcile remote state before retry') from error


def checked(*arguments):
    result = command(*arguments)
    gate.require(result.returncode == 0, 'gh release failed; reconcile remote state before retry: ' + result.stderr.strip())
    return result.stdout


def view(tag):
    result = command('view', tag, '--json', 'tagName,isDraft,isImmutable,isPrerelease,assets')
    # This is the pinned CLI's explicit missing-release result. Auth, transport
    # failures and every other response stop; they must not trigger creation.
    if result.returncode == 1 and result.stderr.strip() == 'release not found':
        return None
    gate.require(result.returncode == 0, 'cannot inspect release; reconcile before retry: ' + result.stderr.strip())
    value = gate.strict_json(result.stdout.encode())
    gate.require(value.get('tagName') == tag and type(value.get('isDraft')) is bool
                 and type(value.get('isImmutable')) is bool
                 and type(value.get('isPrerelease')) is bool
                 and value['isPrerelease'] == ('-' in tag), 'remote release identity/state differs')
    gate.require(value['isDraft'] or value['isImmutable'], 'published release must be immutable')
    return value


def files(root, signatures, metadata):
    root, signatures = Path(root), Path(signatures)
    expected_sidecars = {contract.BUNDLE}
    if 'ghcr' in metadata['channels']:
        expected_sidecars |= {'ghcr-index-provenance.json'}
        expected_sidecars |= {f'ghcr-{arch}-{kind}.json' for arch in ('amd64', 'arm64')
                             for kind in ('provenance', 'image-sbom', 'binary-sbom')}
    gate.require({p.name for p in signatures.iterdir()} == expected_sidecars,
                 'unexpected/missing attestation sidecars')
    paths = [p for directory in (root, signatures) for p in directory.iterdir()]
    gate.require(len({p.name.casefold() for p in paths}) == len(paths)
                 and all(re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]*', p.name) for p in paths),
                 'unsafe or colliding release names')
    for path in paths:
        contract.regular(path)
    return {p.name: p.absolute() for p in paths}


def hashes(paths):
    return {name: gate.digest(contract.regular(path)) for name, path in paths.items()}


def reconcile(tag, remote, expected):
    if remote is None:
        return sorted(expected)
    assets = remote.get('assets')
    gate.require(isinstance(assets, list), 'release assets unavailable')
    names = [a['name'] for a in assets]
    gate.require(len(names) == len(set(names)) and set(names) <= set(expected),
                 'duplicate or unapproved remote release assets')
    if names:
        # Download with the standard client into a fresh directory; hash actual
        # bytes rather than trusting optional API digest fields or file sizes.
        with tempfile.TemporaryDirectory(prefix='hennery-readback-') as temporary:
            checked('download', tag, '--dir', temporary)
            downloaded = {p.name: gate.digest(contract.regular(p)) for p in Path(temporary).iterdir()}
        gate.require(downloaded == {name: expected[name] for name in names}, 'remote release asset bytes differ')
    return sorted(set(expected) - set(names))


def require_public(root, signatures, metadata):
    expected = hashes(files(root, signatures, metadata))
    remote = view(metadata['tag'])
    gate.require(remote is not None and remote['isDraft'] is False and remote['isImmutable'] is True,
                 'channel requires public immutable GitHub release')
    gate.require(not reconcile(metadata['tag'], remote, expected), 'immutable public release is incomplete')


def release(root, signatures, authorize):
    metadata = authorize()
    paths = files(root, signatures, metadata)
    expected = hashes(paths)
    tag = metadata['tag']
    remote = view(tag)
    missing = reconcile(tag, remote, expected)
    gate.require(remote is None or remote['isDraft'] or not missing,
                 'immutable release is incomplete; use a new signed version')

    def boundary():
        gate.require(authorize() == metadata and hashes(paths) == expected,
                     'release authority or bytes changed before write')

    if remote is None:
        boundary()
        notes = ('Source: https://github.com/' + gate.REPOSITORY + '/tree/' + metadata['source_sha']
                 + '\nSHA256SUMS: ' + expected[contract.CHECKSUMS] + '\n')
        with tempfile.TemporaryDirectory(prefix='hennery-notes-') as temporary:
            note_file = Path(temporary) / 'notes.txt'
            note_file.write_text(notes)
            checked('create', tag, '--draft', '--verify-tag', '--target', metadata['source_sha'],
                    '--title', tag, '--notes-file', note_file, '--latest=false',
                    '--prerelease=' + str('-' in tag).lower())
        remote = view(tag)
        gate.require(remote is not None and remote['isDraft'], 'draft creation was not confirmed; reconcile')
    if missing:
        boundary()
        # No --clobber: interrupted parallel uploads resume by remote digest on
        # the next approved attempt. One CLI batch, with no per-asset policy loop.
        checked('upload', tag, *(paths[name] for name in missing))
    gate.require(not reconcile(tag, view(tag), expected), 'release asset set incomplete')
    boundary()
    if remote['isDraft']:
        checked('edit', tag, '--draft=false', '--verify-tag', '--latest=false')
    require_public(root, signatures, metadata)
    return {'state': 'immutable', 'tag': tag, 'checksums_sha256': expected[contract.CHECKSUMS]}
