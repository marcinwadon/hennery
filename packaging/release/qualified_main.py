#!/usr/bin/env python3
"""Promote immutable, fully accepted main bytes for the exact signed source."""
import argparse
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tempfile
import uuid
import zipfile
from urllib.parse import urlencode

import contract
import gate
import release_run
import supply

PROOFS = {'homebrew-proof-macos-15': 'aarch64-apple-darwin',
          'homebrew-proof-ubuntu-24.04': 'x86_64-unknown-linux-musl',
          'homebrew-proof-ubuntu-24.04-arm': 'aarch64-unknown-linux-musl'}


def fresh(api, suffix):
    return api.get(suffix + ('&' if '?' in suffix else '?') + '_=' + uuid.uuid4().hex)


def select(api, source):
    workflow = fresh(api, '/actions/workflows/packaging.yml')
    query = urlencode({'event': 'push', 'branch': 'main', 'head_sha': source, 'per_page': 100})
    runs = fresh(api, '/actions/workflows/packaging.yml/runs?' + query)['workflow_runs']
    exact = [run for run in runs if run.get('head_sha') == source and run.get('head_branch') == 'main'
             and run.get('event') == 'push' and run.get('status') == 'completed'
             and run.get('conclusion') == 'success' and run.get('workflow_id') == workflow['id']
             and run.get('path') == '.github/workflows/packaging.yml'
             and run.get('repository', {}).get('full_name') == gate.REPOSITORY]
    gate.require(len(exact) == 1, 'one successful main packaging run required for the exact signed source')
    run = exact[0]
    artifacts = api.pages(f'/actions/runs/{run["id"]}/artifacts?_=' + uuid.uuid4().hex, 'artifacts')
    names = ['release-payload-' + str(run['id']), *PROOFS]
    selected = {}
    for name in names:
        matches = [a for a in artifacts if a.get('name') == name and not a.get('expired')]
        gate.require(len(matches) == 1, 'missing/ambiguous main acceptance artifact: ' + name)
        artifact = matches[0]
        gate.require(re.fullmatch(r'sha256:[0-9a-f]{64}', artifact.get('digest', '')) is not None,
                     'main artifact lacks immutable digest')
        gate.require(artifact.get('workflow_run', {}).get('head_sha') == source,
                     'main artifact source differs')
        selected[name] = {'id': artifact['id'], 'digest': artifact['digest']}
    return {'schema': 1, 'repository': gate.REPOSITORY, 'source_sha': source,
            'run_id': run['id'], 'run_attempt': run['run_attempt'], 'artifacts': selected}


def download(artifact, destination):
    """The standard gh transport handles authenticated artifact redirects."""
    destination.mkdir()
    with tempfile.TemporaryFile() as archive:
        subprocess.run(['gh', 'api', '--allow-escape-sequences',
                        f'repos/{gate.REPOSITORY}/actions/artifacts/{int(artifact["id"])}/zip?_=' + uuid.uuid4().hex],
                       stdout=archive, check=True)
        archive.seek(0)
        import hashlib
        digest = hashlib.file_digest(archive, 'sha256').hexdigest()
        gate.require('sha256:' + digest == artifact['digest'], 'downloaded main artifact digest differs')
        archive.seek(0)
        with zipfile.ZipFile(archive) as zipped:
            entries = zipped.infolist()
            names = [entry.filename for entry in entries]
            gate.require(len(entries) <= 256 and len({name.casefold() for name in names}) == len(names),
                         'ambiguous or excessive main artifact entries')
            gate.require(sum(entry.file_size for entry in entries) <= 8 * 1024**3, 'main artifact too large')
            for entry in entries:
                mode = entry.external_attr >> 16
                gate.require(re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]*', entry.filename) is not None
                             and not entry.is_dir() and (not stat.S_IFMT(mode) or stat.S_ISREG(mode)),
                             'nonregular or unsafe main artifact member')
                with zipped.open(entry) as source, (destination / entry.filename).open('xb') as target:
                    shutil.copyfileobj(source, target)


def promote(candidate, proofs, output, receipt, value, tag, source, advisories=None):
    metadata = contract.load(candidate)
    gate.require(metadata['source_sha'] == source and metadata['tag'] == tag
                 and metadata['tag_object'] == '0' * 40, 'main candidate tag/source binding differs')
    gate.require(metadata['channels'] == value['channels'] and metadata['targets'] == sorted(gate.TARGETS),
                 'main candidate lacks complete channel/target qualification')
    supply.require_contract(candidate, value['channels'], metadata['targets'], tag)
    supply.verify_binary_inventory(metadata, candidate)
    checksums = gate.digest(contract.regular(candidate / contract.CHECKSUMS))
    for name, target in PROOFS.items():
        proof = gate.strict_json(contract.regular(proofs / name / 'result.json'))
        gate.require(proof.get('source_sha') == source and proof.get('native') == target
                     and proof.get('checksums_sha256') == checksums
                     and proof.get('install') == tag[1:] and proof.get('upgrade') == tag[1:]
                     and all(proof.get(key) is True for key in
                             ('uninstall', 'binary_unchanged', 'data_and_service_preserved')),
                     'main native acceptance proof differs: ' + name)
    output.mkdir()
    for path in candidate.iterdir():
        if path.name not in (contract.CHECKSUMS, contract.METADATA):
            (output / path.name).write_bytes(contract.regular(path))
    (output / 'qualified-main-inputs.json').write_bytes(gate.canonical(receipt | {'checksums_sha256': checksums}))
    if advisories is not None:
        # Fresh tag-time audits gate prepare; preserve their exact reports in
        # the attested closure without modifying any accepted public archive.
        reports = {p.name: contract.regular(p).decode() for p in advisories.iterdir()}
        gate.require({'rust-advisories.json', 'rust-advisory-revision.txt', 'web-advisories.json'} <= set(reports),
                     'fresh tag audit reports missing')
        (output / 'tag-advisories.json').write_bytes(gate.canonical(reports))
    release_run.freeze_payload(output, value, tag, source)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--advisories', type=Path, required=True)
    args = parser.parse_args()
    value = release_run.configuration()
    source, tag = os.environ['GITHUB_SHA'], os.environ['GITHUB_REF_NAME']
    subprocess.run(['git', 'merge-base', '--is-ancestor', source, 'refs/remotes/origin/main'], check=True)
    receipt = select(gate.GitHub(os.environ['GH_TOKEN']), source)
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for name, artifact in receipt['artifacts'].items():
            download(artifact, work / name)
        promote(work / ('release-payload-' + str(receipt['run_id'])), work, args.output,
                receipt, value, tag, source, args.advisories)


if __name__ == '__main__':
    main()
