"""Signed-source, checksums and provenance for final release bytes."""
import re
from pathlib import Path
import subprocess

import gate
import verifier_process

CHECKSUMS = 'SHA256SUMS'
METADATA = 'release-metadata.json'
BUNDLE = 'release.sigstore.jsonl'
WORKFLOW = '.github/workflows/release.yml'
TAG = r'v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?'
SHA = r'[0-9a-f]{40}'


def parse_checksums(raw):
    gate.require(isinstance(raw, bytes) and raw.endswith(b'\n'), 'checksum list must end with newline')
    try:
        lines = raw.decode('ascii').splitlines()
    except UnicodeError as error:
        raise gate.Refused('checksum list must be ASCII') from error
    entries, folded = {}, set()
    for line in lines:
        match = re.fullmatch(r'([0-9a-f]{64})  ([A-Za-z0-9][A-Za-z0-9._-]*)', line)
        gate.require(match is not None, 'malformed checksum entry')
        digest, name = match.groups()
        gate.require(name.casefold() not in (CHECKSUMS.casefold(), BUNDLE.casefold()) and name.casefold() not in folded,
                     'duplicate, ambiguous or cyclic checksum entry')
        entries[name] = digest
        folded.add(name.casefold())
    gate.require(entries and list(entries) == sorted(entries), 'checksum list must be nonempty and sorted')
    gate.require(raw == ''.join(f'{sha}  {name}\n' for name, sha in entries.items()).encode('ascii'),
                 'checksum list must use canonical lines')
    return entries


def attestation_command(subject, tag, source_sha, bundle, trusted_root=None):
    gate.require(isinstance(tag, str) and re.fullmatch(TAG, tag) and isinstance(source_sha, str)
                 and re.fullmatch(SHA, source_sha), 'invalid attestation tag/source binding')
    command = ['gh', 'attestation', 'verify', str(subject), '--repo', gate.REPOSITORY,
               '--cert-identity', f'https://github.com/{gate.REPOSITORY}/{WORKFLOW}@refs/tags/{tag}',
               '--source-ref', 'refs/tags/' + tag, '--source-digest', source_sha, '--signer-digest', source_sha,
               '--cert-oidc-issuer', 'https://token.actions.githubusercontent.com',
               '--predicate-type', 'https://slsa.dev/provenance/v1', '--deny-self-hosted-runners']
    if bundle is not None:
        command += ['--bundle', str(bundle)]
    if trusted_root is not None:
        command += ['--custom-trusted-root', str(trusted_root)]
    return command


def regular(path):
    path = Path(path)
    gate.require(path.is_file() and not path.is_symlink(), 'missing/nonregular release file: ' + path.name)
    return path.read_bytes()


def verify_files(root):
    root = Path(root)
    gate.require(root.is_dir() and not root.is_symlink(), 'nonregular release directory')
    entries = parse_checksums(regular(root / CHECKSUMS))
    actual = {}
    for path in root.iterdir():
        data = regular(path)
        if path.name != CHECKSUMS:
            actual[path.name] = gate.digest(data)
    gate.require(actual == entries, 'release checksum closure incomplete or bytes changed')
    return entries


def validate_metadata(value):
    gate.require(isinstance(value, dict) and value.get('schema') in (2, 3) and value.get('repository') == gate.REPOSITORY,
                 'invalid release metadata repository/schema')
    # Schema 2 is retained for alpha.3; its attesting field is historical data,
    # never an authorization input. New releases write schema 3 without it.
    required = {'schema', 'repository', 'source_sha', 'tag', 'tag_object', 'channels', 'targets'}
    gate.require(required <= set(value) <= required | {'binaries'} | ({'attesting'} if value['schema'] == 2 else set()), 'unexpected release metadata fields')
    gate.require(isinstance(value.get('tag'), str) and re.fullmatch(TAG, value['tag']), 'invalid release metadata tag')
    for field in ('source_sha', 'tag_object'):
        gate.require(isinstance(value.get(field), str) and re.fullmatch(SHA, value[field]), 'invalid release metadata ' + field)
    for field, allowed in [('channels', gate.CHANNELS), ('targets', gate.TARGETS)]:
        items = value.get(field)
        gate.require(isinstance(items, list) and items and all(isinstance(x, str) for x in items)
                     and items == sorted(set(items)) and set(items) <= allowed, 'invalid release metadata ' + field)
    gate.require('github' in value['channels'] and ('-' not in value['tag'] or value['channels'] == ['github']),
                 'prerelease channels must remain GitHub only')
    return value


def freeze(root, metadata):
    root = Path(root)
    gate.require(not any((root / name).exists() or (root / name).is_symlink() for name in (METADATA, CHECKSUMS, BUNDLE)),
                 'release already frozen or cyclic sidecar present')
    validate_metadata(metadata)
    (root / METADATA).write_bytes(gate.canonical(metadata))
    entries = [(p.name, gate.digest(regular(p))) for p in sorted(root.iterdir())]
    raw = ''.join(f'{digest}  {name}\n' for name, digest in entries).encode('ascii')
    parse_checksums(raw)
    (root / CHECKSUMS).write_bytes(raw)
    verify_files(root)
    return gate.digest(raw)


def load(root):
    root = Path(root)
    entries = verify_files(root)
    value = validate_metadata(gate.strict_json(regular(root / METADATA)))
    # Derived view for channel validators. SHA256SUMS remains the only global
    # digest list on disk; metadata contains no duplicated payload inventory.
    return value | {'artifacts': [{'name': name, 'sha256': digest, 'size': len(regular(root / name))}
                                  for name, digest in entries.items()]}


def verify(root, tag, source_sha, bundle, trusted_root=None):
    import tag_signature
    root = Path(root)
    value = load(root)
    gate.require(value['tag'] == tag and value['source_sha'] == source_sha, 'release tag/source binding differs')
    tag_signature.verify(tag, source_sha, value['tag_object'])
    subjects = [CHECKSUMS] + [item['name'] for item in value['artifacts']]
    for subject in subjects:
        command = attestation_command(root / subject, tag, source_sha, bundle, trusted_root)
        verifier_process.run(command, check=True, capture_output=True, text=True)
    gate.require(load(root) == value, 'release bytes changed during verification')
    return value
