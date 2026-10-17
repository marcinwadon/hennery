"""Inactive pre-migration publisher; not imported by the release runtime.

Retained until operator-authorized hosted qualification of hosting.py proves
OIDC, approval, original artifacts and immutable remote readback. Do not wire
this back into the workflow as a fallback; then delete with publish.py.
"""
from pathlib import Path
import urllib.parse

import contract
import gate
import publish as transport


def release(root, signatures, authorize):
    root, signatures = Path(root), Path(signatures)
    metadata = authorize()
    files = {p.name: p for p in root.iterdir()}
    sidecars = {p.name: p for p in signatures.iterdir()}
    expected_sidecars = {contract.BUNDLE}
    if 'ghcr' in metadata['channels']:
        expected_sidecars |= {'ghcr-index-provenance.json'}
        expected_sidecars |= {f'ghcr-{arch}-{kind}.json' for arch in ('amd64', 'arm64')
                             for kind in ('provenance', 'image-sbom', 'binary-sbom')}
    gate.require(set(sidecars) == expected_sidecars, 'unexpected/missing attestation sidecars')
    files.update(sidecars)
    expected = {name: gate.digest(contract.regular(path)) for name, path in files.items()}
    tag_path = f'/repos/{gate.REPOSITORY}/releases/tags/' + urllib.parse.quote(metadata['tag'], safe='')
    remote = transport.api(tag_path)

    def reconcile(release):
        if release is None:
            return {'complete': [], 'missing': sorted(expected)}
        gate.require(release.get('tag_name') == metadata['tag'], 'remote release tag differs')
        gate.require(release.get('draft') is True or (release.get('draft') is False and release.get('immutable') is True),
                     'published release must be immutable')
        return transport.plan(expected, transport.release_assets(release))

    state = reconcile(remote)
    gate.require(remote is None or remote.get('draft') is True or not state['missing'],
                 'immutable release is incomplete; use a new signed version')

    def boundary():
        gate.require(authorize() == metadata, 'release authorization changed at write boundary')
        gate.require({name: gate.digest(contract.regular(path)) for name, path in files.items()} == expected,
                     'release or bundle bytes changed at write boundary')

    if remote is None:
        boundary()
        remote = transport.api(f'/repos/{gate.REPOSITORY}/releases', method='POST', data=gate.canonical({
            'tag_name': metadata['tag'], 'target_commitish': metadata['source_sha'], 'name': metadata['tag'],
            'draft': True, 'prerelease': '-' in metadata['tag'], 'make_latest': 'false',
            'body': 'Source: https://github.com/' + gate.REPOSITORY + '/tree/' + metadata['source_sha']
                    + '\nSHA256SUMS: ' + expected[contract.CHECKSUMS]}))
        gate.require(isinstance(remote, dict) and remote.get('draft') is True and type(remote.get('id')) is int,
                     'draft creation was not acknowledged; stop and reconcile')
    for name in state['missing']:
        boundary()
        transport.api(f'/repos/{gate.REPOSITORY}/releases/{remote["id"]}/assets?name=' + urllib.parse.quote(name, safe=''),
                      method='POST', data=contract.regular(files[name]), content_type='application/octet-stream')
        gate.require(name in reconcile(remote)['complete'], 'uploaded bytes failed read-back verification')
    gate.require(not reconcile(remote)['missing'], 'release asset set incomplete')
    # A second full cryptographic verification authenticates the read-back bytes:
    # plan() requires byte equality, and authorize() rechecks those exact inputs.
    boundary()
    if remote.get('draft') is True:
        remote = transport.api(f'/repos/{gate.REPOSITORY}/releases/{remote["id"]}', method='PATCH',
                               data=gate.canonical({'draft': False, 'make_latest': 'false'}))
    gate.require(isinstance(remote, dict) and remote.get('draft') is False and remote.get('immutable') is True,
                 'publication did not establish immutability')
    published = transport.api(tag_path)
    gate.require(isinstance(published, dict) and published.get('draft') is False and published.get('immutable') is True,
                 'final public read-back did not confirm immutability')
    gate.require(not reconcile(published)['missing'], 'public immutable release is incomplete')
    return {'state': 'immutable', 'tag': metadata['tag'], 'checksums_sha256': expected[contract.CHECKSUMS]}
