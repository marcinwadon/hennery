#!/usr/bin/env python3
"""Authenticate release bytes and OCI subjects with signed source and provenance."""
import argparse
from pathlib import Path
import re
import subprocess

import assets
import attestations
import contract
import verifier_process
import gate
import supply
import tag_signature

PROVENANCE = 'https://slsa.dev/provenance/v1'
SPDX = 'https://spdx.dev/Document'


def attestation_command(subject, tag, source_sha, bundle=None, trusted_root=None):
    return contract.attestation_command(subject, tag, source_sha, bundle, trusted_root)


def verify(root, bundle, source_sha, tag, trusted_root=None):
    root = Path(root)
    value = contract.verify(root, tag, source_sha, bundle, trusted_root)
    supply.verify_binary_inventory(value, root)
    gate.require(contract.load(root) == value, 'release bytes changed during binary verification')
    return value


def verify_public(root, bundle, source_sha, tag, trusted_root=None):
    root = Path(root)
    entries = contract.verify_files(root)
    gate.require(set(entries) == set(assets.names(sorted(gate.TARGETS))), 'public release asset inventory differs')
    tag_signature.verify(tag, source_sha)
    for name in [contract.CHECKSUMS, *entries]:
        subject = root / name
        with attestations.bundle(gate.digest(contract.regular(subject)), bundle) as proof:
            verifier_process.run(attestation_command(subject, tag, source_sha, proof, trusted_root),
                                 check=True, capture_output=True, text=True)
    gate.require(contract.verify_files(root) == entries, 'public release bytes changed during verification')
    return {'tag': tag, 'source_sha': source_sha, 'targets': sorted(gate.TARGETS)}


def image_descriptor(root, value):
    index = gate.strict_json(contract.regular(Path(root) / 'ghcr-index.json'))
    gate.require('ghcr' in value['channels'] and index.get('schema') == 1
                 and index.get('source_sha') == value['source_sha']
                 and index.get('version') == value['tag'].removeprefix('v')
                 and isinstance(index.get('platforms'), dict) and set(index['platforms']) == {'amd64','arm64'},
                 'invalid release image descriptor')
    digests = [index.get('digest'), *index['platforms'].values()]
    gate.require(all(isinstance(d, str) and re.fullmatch(r'sha256:[0-9a-f]{64}', d) for d in digests)
                 and len(set(digests)) == 3, 'invalid release image digest set')
    return index


def verify_image_subject(image, value, bundle=None, predicate_type=PROVENANCE, manifest=None, trusted_root=None):
    gate.require(re.fullmatch(r'ghcr\.io/marcinwadon/hennery@sha256:[0-9a-f]{64}', image), 'verification requires an immutable image digest')
    gate.require(predicate_type in (PROVENANCE, SPDX), 'unexpected attestation predicate')
    subject = 'oci://' + image
    if manifest is not None:
        manifest = Path(manifest)
        gate.require(bundle is not None and 'sha256:' + gate.digest(contract.regular(manifest)) == image.split('@',1)[1],
                     'local manifest must match the release image digest and have a bundle')
        subject = manifest
    command = attestation_command(subject, value['tag'], value['source_sha'], bundle, trusted_root)
    command[command.index('--predicate-type') + 1] = predicate_type
    if bundle is None: command += ['--bundle-from-oci']
    verifier_process.run(command, check=True, capture_output=True, text=True)


def verify_image(image, tag, bundle=None, predicate_type=PROVENANCE, release_directory=None,
                 release_bundle=None, manifest=None, source_sha=None, trusted_root=None):
    gate.require(re.fullmatch(r'ghcr\.io/marcinwadon/hennery@sha256:[0-9a-f]{64}', image), 'verification requires an immutable image digest')
    gate.require(release_directory is not None and source_sha is not None,
                 'image requires checksum-authenticated release and source binding')
    root = Path(release_directory)
    if not (root / contract.METADATA).exists():
        value = verify_public(root, release_bundle, source_sha, tag, trusted_root)
        with attestations.bundle(image.split('@sha256:')[1], bundle) as proof:
            verify_image_subject(image, value, proof, predicate_type, manifest, trusted_root)
        return value
    value = contract.load(root)
    index = image_descriptor(root, value)
    gate.require(image.split('@',1)[1] in [index['digest'], *index['platforms'].values()], 'image digest is not in release descriptor')
    verified = verify(root, release_bundle, source_sha, tag, trusted_root)
    gate.require(verified == value, 'release changed before image verification')
    verify_image_subject(image, value, bundle, predicate_type, manifest, trusted_root)
    gate.require(contract.load(root) == value, 'release bytes changed during image verification')
    return value


def require_binstall_metadata(metadata, tag):
    # Native binstall has no exact workflow/source verifier. These mappings
    # are dormant until a separately reviewed bridge implements this contract.
    disabled = metadata.get('binstall', {}).get('disabled-strategies')
    gate.require(isinstance(disabled, list) and sorted(disabled) == ['compile','crate-meta-data','quick-install'],
                 'native binstall strategies must remain disabled without an identity-verifying bridge')
    policy = metadata.get('hennery-release', {})
    command = attestation_command('unused', tag, '0' * 40)
    gate.require(policy.get('verification') == 'external-signed-tag-sigstore'
                 and policy.get('maintainer-fingerprint') == tag_signature.FINGERPRINT
                 and policy.get('signer-identity') == command[command.index('--cert-identity') + 1],
                 'binstall external verification metadata must pin the exact release-tag identity')


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument('--release-directory', type=Path, required=True)
    parser.add_argument('--bundle', type=Path, help='optional cached provenance bundle; otherwise fetch public GitHub attestations by digest')
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--trusted-root', type=Path)
    parser.add_argument('--image')
    parser.add_argument('--image-bundle', type=Path)
    parser.add_argument('--manifest', type=Path)
    parser.add_argument('--predicate-type', choices=[PROVENANCE, SPDX], default=PROVENANCE)
    return parser.parse_args(argv)


def main():
    args = arguments()
    if args.image:
        verify_image(args.image, args.tag, args.image_bundle, args.predicate_type, args.release_directory,
                     args.bundle, args.manifest, args.source_sha, args.trusted_root)
    else:
        if (args.release_directory / contract.METADATA).exists():
            verify(args.release_directory, args.bundle, args.source_sha, args.tag, args.trusted_root)
        else:
            verify_public(args.release_directory, args.bundle, args.source_sha, args.tag, args.trusted_root)


if __name__ == '__main__':
    try: main()
    except (gate.Refused, KeyError, OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit('release verification refused: ' + str(error)) from error
