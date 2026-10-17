"""The pinned CLI verifies genuine alpha.3 bytes offline, under production flags."""
import hashlib
from pathlib import Path
import subprocess
import tempfile
import unittest

import contract
import gate
import verifier_process

FIXTURES = Path(__file__).with_name('fixtures')
ALPHA = FIXTURES / 'alpha3'
TRUST = FIXTURES / 'attestation/trusted_root.json'
HASHES = {'SHA256SUMS': '044729948b43f415d4ebf71cbe4469d8521c68ad14c2d663907a34a52c3bb08b', 'release-metadata.json': '10c3ff9e53b31b95dae543ddbd08d6a86d8b97363e6a1508e0d4f527216feb64', 'release.sigstore.jsonl': 'a3f768af3af38ef2655bde54891dbdefa60d5b9e96a0e39454d947fd54d41f31'}


class PinnedCLI(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if hashlib.sha256(TRUST.read_bytes()).hexdigest() != '455b3fe53e2678889f093d66ebbfda5f12ebb9d682b146c3535fa04e97689787':
            raise AssertionError('offline Sigstore trust-root fixture changed')
        for name, digest in HASHES.items():
            if hashlib.sha256((ALPHA / name).read_bytes()).hexdigest() != digest:
                raise AssertionError('genuine alpha.3 fixture changed: ' + name)
        cls.metadata = gate.strict_json((ALPHA / contract.METADATA).read_bytes())
        isolated = verifier_process.environment()
        cls.env = isolated.__enter__()
        cls.addClassCleanup(isolated.__exit__, None, None, None)
        # Any accidental fetch must fail; no account, token or network gh call.
        cls.env.update(HTTPS_PROXY='http://127.0.0.1:9', HTTP_PROXY='http://127.0.0.1:9',
                       ALL_PROXY='http://127.0.0.1:9', NO_PROXY='')
        version = subprocess.check_output(['gh', '--version'], env=cls.env, text=True)
        if not version.startswith('gh version 2.101.0 '):
            raise AssertionError('requalify the real consumer boundary before changing gh: ' + version)

    def command(self, subject=contract.CHECKSUMS):
        return contract.attestation_command(ALPHA / subject, self.metadata['tag'], self.metadata['source_sha'],
                                            ALPHA / contract.BUNDLE, TRUST)

    def invoke(self, command):
        return subprocess.run(command, env=self.env, capture_output=True, text=True, timeout=30)

    def test_genuine_hennery_release_passes_supported_digest_source_tag_policy(self):
        contract.validate_metadata(self.metadata)  # Legacy schema 2 remains consumable.
        checksums = contract.parse_checksums((ALPHA / contract.CHECKSUMS).read_bytes())
        self.assertEqual(checksums[contract.METADATA], gate.digest((ALPHA / contract.METADATA).read_bytes()))
        for subject in (contract.CHECKSUMS, contract.METADATA):
            with self.subTest(subject=subject):
                result = self.invoke(self.command(subject))
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_tampered_real_payload_fails_before_use(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / contract.CHECKSUMS
            path.write_bytes((ALPHA / contract.CHECKSUMS).read_bytes() + b'changed')
            command = self.command(); command[3] = str(path)
            self.assertNotEqual(self.invoke(command).returncode, 0)

    def test_wrong_requested_source_or_tag_cannot_accept_the_real_release(self):
        for tag, source in [(self.metadata['tag'], 'b' * 40),
                            ('v0.1.0-alpha.2', self.metadata['source_sha'])]:
            with self.subTest(tag=tag, source=source):
                command = contract.attestation_command(ALPHA / contract.CHECKSUMS, tag, source,
                                                        ALPHA / contract.BUNDLE, TRUST)
                self.assertNotEqual(self.invoke(command).returncode, 0)

    def test_wrong_identity_source_tag_or_predicate_fails_real_crypto_policy(self):
        mutations = {'--repo': 'another/repository', '--source-ref': 'refs/heads/main',
                     '--source-digest': 'b' * 40, '--signer-digest': 'b' * 40,
                     '--cert-identity': 'https://github.com/marcinwadon/hennery/.github/workflows/other.yml@refs/tags/v0.1.0-alpha.3',
                     '--cert-oidc-issuer': 'https://invalid.example', '--predicate-type': 'https://spdx.dev/Document'}
        for flag, value in mutations.items():
            with self.subTest(flag=flag):
                command = self.command(); command[command.index(flag) + 1] = value
                self.assertNotEqual(self.invoke(command).returncode, 0)
        command = self.command()
        command[command.index('--cert-identity') + 1] = 'https://github.com/marcinwadon/hennery/.github/workflows/release.yml@refs/tags/v0.1.0-alpha.2'
        self.assertNotEqual(self.invoke(command).returncode, 0)
