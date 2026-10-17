"""Consumer lifecycle and structural identity tests; real crypto is in test_verify_cli."""
import copy
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import contract
import gate
import tag_signature
import verify


class Verification(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(); self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / 'installer.sh').write_bytes(b'fixture installer')
        self.metadata = {'schema': 3, 'repository': gate.REPOSITORY, 'source_sha': 'a' * 40,
                         'tag': 'v0.1.0', 'tag_object': 'b' * 40, 'channels': ['github'],
                         'targets': sorted(gate.TARGETS)}
        contract.freeze(self.root, self.metadata)
        self.identity = 'https://github.com/' + gate.REPOSITORY + '/' + contract.WORKFLOW + '@refs/tags/v0.1.0'
        self.calls = []

    def crypto(self, command, **kwargs):
        self.calls.append(command)
        return subprocess.CompletedProcess(command, 0, 'verified', '')

    def run_verify(self, **kwargs):
        return verify.verify(self.root, Path('bundle'), 'a' * 40, 'v0.1.0', **kwargs)

    def test_signed_tag_precedes_checksum_and_all_payload_verification(self):
        with patch('tag_signature.verify') as tag, patch.object(verify.subprocess, 'run', side_effect=self.crypto), patch.object(verify.supply, 'verify_binary_inventory') as binaries:
            actual = self.run_verify()
        tag.assert_called_once_with('v0.1.0', 'a' * 40, 'b' * 40)
        binaries.assert_called_once_with(actual, self.root)
        self.assertEqual([Path(c[3]).name for c in self.calls], ['SHA256SUMS', 'installer.sh', 'release-metadata.json'])
        for command in self.calls:
            self.assertNotIn('--signer-workflow', command)
            self.assertEqual(command[command.index('--cert-identity') + 1], self.identity)
            self.assertEqual(command[command.index('--source-digest') + 1], 'a' * 40)
            self.assertEqual(command[command.index('--signer-digest') + 1], 'a' * 40)

    def test_tag_or_crypto_authentication_failure_stops_before_installer(self):
        with patch('tag_signature.verify', side_effect=gate.Refused('invalid tag')), patch.object(verify.subprocess, 'run') as crypto:
            with self.assertRaisesRegex(gate.Refused, 'invalid tag'): self.run_verify()
            crypto.assert_not_called()
        with patch('tag_signature.verify'), patch.object(verify.subprocess, 'run', side_effect=subprocess.CalledProcessError(1, 'gh')):
            with self.assertRaises(subprocess.CalledProcessError): self.run_verify()

    def test_source_and_tag_bindings_cannot_change(self):
        for source, tag in [('c' * 40, 'v0.1.0'), ('a' * 40, 'v0.2.0')]:
            with self.subTest(source=source, tag=tag), patch.object(verify.subprocess, 'run') as crypto, self.assertRaises(gate.Refused):
                verify.verify(self.root, Path('bundle'), source, tag)
            crypto.assert_not_called()

    def test_byte_change_during_binary_check_refuses(self):
        def mutate(value, root):
            (root / 'installer.sh').write_bytes(b'changed after attestation')
        with patch('tag_signature.verify'), patch.object(verify.subprocess, 'run', side_effect=self.crypto), patch.object(verify.supply, 'verify_binary_inventory', side_effect=mutate), self.assertRaises(gate.Refused): self.run_verify()

    def test_cli_parser_requires_release_source_and_tag(self):
        args = verify.arguments(['--release-directory', str(self.root), '--bundle', 'bundle', '--source-sha', 'a' * 40,
                                 '--tag', 'v0.1.0'])
        self.assertEqual(args.tag, 'v0.1.0')
        self.assertFalse(hasattr(args, 'run_id'))


class BinstallMetadata(unittest.TestCase):
    def test_metadata_cannot_advertise_an_unverified_or_branch_signed_path(self):
        good = {'binstall': {'disabled-strategies': ['crate-meta-data','quick-install','compile']},
                'hennery-release': {'verification': 'external-signed-tag-sigstore',
                 'maintainer-fingerprint': tag_signature.FINGERPRINT,
                 'signer-identity': 'https://github.com/marcinwadon/hennery/.github/workflows/release.yml@refs/tags/v0.1.0'}}
        verify.require_binstall_metadata(good, 'v0.1.0')
        for kind in ('native','branch','wrong-tag','wrong-maintainer','no-verifier'):
            bad = copy.deepcopy(good)
            if kind == 'native': bad['binstall']['disabled-strategies'].remove('crate-meta-data')
            elif kind == 'branch': bad['hennery-release']['signer-identity'] = bad['hennery-release']['signer-identity'].replace('@refs/tags/v0.1.0','@refs/heads/main')
            elif kind == 'wrong-tag': bad['hennery-release']['signer-identity'] += '-other'
            elif kind == 'wrong-maintainer': bad['hennery-release']['maintainer-fingerprint'] = '0' * 40
            else: bad['hennery-release']['verification'] = 'checksum-only'
            with self.subTest(kind=kind), self.assertRaises(gate.Refused): verify.require_binstall_metadata(bad,'v0.1.0')


if __name__ == '__main__': unittest.main()
