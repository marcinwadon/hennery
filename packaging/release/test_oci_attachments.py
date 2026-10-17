"""Real ORAS producer/consumer output, using an offline OCI layout and no account."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import channel_publish
import contract
import verifier_process


class OCIAttachments(unittest.TestCase):
    def test_standard_attachment_retains_the_subject_and_actual_bundle_bytes(self):
        with tempfile.TemporaryDirectory() as tmp, verifier_process.environment() as env:
            root = Path(tmp)
            layout = root / 'layout'
            payload = root / 'payload'; payload.write_bytes(b'OCI subject fixture')
            actual = subprocess.run
            def oras(*args):
                return actual(['oras', *args], env=env, cwd=root, check=True, capture_output=True, text=True)
            pushed = json.loads(oras('push', '--oci-layout', str(layout) + ':subject', payload.name,
                                     '--format', 'json').stdout)
            digest = pushed['digest']
            bundle = root / 'bundle.json'
            original = (Path(__file__).with_name('fixtures') / 'alpha3' / contract.BUNDLE).read_bytes()
            bundle.write_bytes(original)
            # Exercise the actual production command, redirecting only its fixed
            # registry transport to a disposable offline OCI layout.
            def transport(command, **kwargs):
                self.assertEqual(command[:2], ['oras', 'attach'])
                self.assertEqual(command[command.index('--registry-config') + 1], 'unused-offline-auth')
                self.assertEqual(command[-2], 'ghcr.io/marcinwadon/hennery@' + digest)
                index = command.index('--registry-config')
                command = command[:index] + ['--oci-layout'] + command[index + 2:]
                command[-2] = str(layout) + '@' + digest
                return actual(command, **kwargs)
            with patch.object(channel_publish.subprocess, 'run', side_effect=transport):
                channel_publish.attach_bundle(digest, 'https://slsa.dev/provenance/v1', bundle,
                                               'unused-offline-auth', env)
            refs = json.loads(oras('discover', '--oci-layout', str(layout) + '@' + digest,
                                  '--format', 'json').stdout)['referrers']
            self.assertEqual(len(refs), 1)
            manifest = json.loads(oras('manifest', 'fetch', '--oci-layout', str(layout) + '@' + refs[0]['digest']).stdout)
            self.assertEqual(manifest['subject']['digest'], digest)
            self.assertEqual(manifest['artifactType'], 'application/vnd.dev.sigstore.bundle.v0.3+json')
            self.assertEqual(manifest['annotations']['dev.sigstore.bundle.predicateType'], 'https://slsa.dev/provenance/v1')
            fetched = root / 'fetched.json'
            oras('blob', 'fetch', '--oci-layout', str(layout) + '@' + manifest['layers'][0]['digest'], '--output', str(fetched))
            self.assertEqual(fetched.read_bytes(), original)
