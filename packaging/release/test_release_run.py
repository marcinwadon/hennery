import contextlib
import io
import os
from pathlib import Path
import tempfile
import tomllib
import unittest
from unittest.mock import patch

import contract
import gate
import release_run
import test_contract


class Orchestrator(unittest.TestCase):
    def test_configuration_binds_original_signed_tag_and_checked_source(self):
        version=tomllib.loads((release_run.ROOT/'Cargo.toml').read_text())['workspace']['package']['version']
        env={'GITHUB_REPOSITORY':gate.REPOSITORY,'GITHUB_EVENT_NAME':'push','GITHUB_REF':'refs/tags/v'+version,
             'GITHUB_REF_NAME':'v'+version,'GITHUB_SHA':'a'*40,'GITHUB_RUN_ATTEMPT':'1'}
        with patch.dict(os.environ,env,clear=True),patch.object(release_run.tag_signature,'verify',return_value={'object_sha':'b'*40}):
            self.assertEqual(release_run.configuration()['channels'],['github'])
            for field,value in [('GITHUB_RUN_ATTEMPT','2'),('GITHUB_REF','refs/heads/main'),('GITHUB_REPOSITORY','other/repo'),('GITHUB_EVENT_NAME','workflow_dispatch'),('GITHUB_REF_NAME','v9.9.9')]:
                with self.subTest(field=field),patch.dict(os.environ,{field:value}),self.assertRaises(gate.Refused):release_run.configuration()

    def test_actual_release_metadata_has_fixed_channels_and_native_publisher_context(self):
        value = gate.strict_json((Path(__file__).with_name('fixtures') / 'alpha3' / contract.METADATA).read_bytes())
        self.assertEqual(release_run.channels(value['tag']), ['github'])
        self.assertEqual(release_run.channels(value['tag'].split('-')[0]), ['crates', 'ghcr', 'github', 'homebrew'])
        env = {'GITHUB_REPOSITORY': gate.REPOSITORY, 'GITHUB_EVENT_NAME': 'push',
               'GITHUB_REF': 'refs/tags/' + value['tag'], 'GITHUB_SHA': value['source_sha'],
               'GITHUB_WORKFLOW_REF': gate.REPOSITORY + '/' + contract.WORKFLOW + '@refs/tags/' + value['tag'],
               'RELEASE_PUBLICATION_ENABLED': 'true', 'RELEASE_GITHUB_ENABLED': 'true'}
        with patch.dict(os.environ, env, clear=True):
            release_run.publication_context(value)
            for field in env:
                with self.subTest(field=field), patch.dict(os.environ, {field: 'untrusted'}), self.assertRaises(gate.Refused):
                    release_run.publication_context(value)

    def test_summary_lists_every_asset_without_draft(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)/'payload';root.mkdir();test_contract.Contract().fixture(root)
            signatures=Path(tmp)/'signatures';signatures.mkdir();(signatures/contract.BUNDLE).write_bytes(b'bundle')
            output=io.StringIO()
            with patch.dict(os.environ,{'GITHUB_RUN_ID':'42','GITHUB_RUN_ATTEMPT':'1'}),contextlib.redirect_stdout(output):
                release_run.summary(root,signatures)
            for directory in (root,signatures):
                for p in directory.iterdir():self.assertIn(p.name,output.getvalue());self.assertIn(gate.digest(p.read_bytes()),output.getvalue())
            self.assertIn('No draft exists',output.getvalue())


    def test_attestation_check_refuses_rerun_before_signing(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);test_contract.Contract().fixture(root)
            with patch.dict(os.environ,{'GITHUB_RUN_ID':'42','GITHUB_RUN_ATTEMPT':'2'}),self.assertRaises(gate.Refused):
                release_run.check_original(root)
