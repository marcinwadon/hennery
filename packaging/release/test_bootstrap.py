"""Execute the exact README bootstrap with real disposable signed-tag fixtures."""
import os
from pathlib import Path
import re
import subprocess
import sys
import unittest

from installer_fixtures import ConsumerFixtures, FINGERPRINT, ROOT, REFUSALS

SCRIPT=None


def documented_script():
    environment=dict(os.environ);environment.pop('README_BLOCKS_AWK',None)
    result=subprocess.run(['sh',str(ROOT/'packaging/readme-blocks.sh'),str(ROOT/'README.md'),'release-bootstrap'],env=environment,capture_output=True,text=True)
    if result.returncode: raise AssertionError('guarded bootstrap missing: '+result.stderr)
    if SCRIPT is not None: assert SCRIPT.read_text()==result.stdout,'workflow must execute the exact README block'
    return result.stdout


class Bootstrap(ConsumerFixtures,unittest.TestCase):
    def fixture(self, case, script, **kwargs):
        tag=re.search(r'^release_tag=(v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?)$',script,re.M)
        self.assertIsNotNone(tag,'README must select an explicit release tag')
        return super().fixture(case,script,tag=tag[1],**kwargs)

    def test_signed_source_bootstrap_contract(self):
        script=documented_script()
        self.assertEqual(script.count('release_fingerprint='+FINGERPRINT),1)
        for old in ('release-inventory','--signer-workflow','verify.py','maintainer.asc'):
            self.assertNotIn(old,script)
        for required in ('SHA256SUMS','verify-tag','release.yml@refs/tags/','--source-digest','--signer-digest'):
            self.assertIn(required,script)

    def test_positive_with_each_documented_sha256_tool(self):
        for tag in ('v0.1.0-alpha.1','v0.1.0-alpha.2','v0.1.0'):
            script=re.sub(r'^release_tag=.*$', 'release_tag='+tag,documented_script(),flags=re.M)
            for tool in ('sha256sum','shasum'):
                with self.subTest(tag=tag,tool=tool):
                    result,marker=self.fixture('valid',script,hash_tool=tool)
                    self.assertEqual(result.returncode,0,result.stdout+result.stderr)
                    self.assertEqual(marker.read_text(),'executed')

    def test_every_refusal_never_executes_installer(self):
        for case in REFUSALS+('wrong-github-key','wrong-keyserver-key','wrong-main-pin'):
            with self.subTest(case=case):
                result,marker=self.fixture(case,documented_script())
                self.assertNotEqual(result.returncode,0,'accepted '+case)
                self.assertFalse(marker.exists(),'executed installer for '+case)

    def test_refusals_do_not_depend_on_shell_errexit(self):
        for tool in ('sha256sum','shasum'):
            for case in ('unsigned-tag','tampered-checksums','tampered-installer','duplicate-checksum'):
                with self.subTest(case=case,tool=tool):
                    result,marker=self.fixture(case,documented_script(),hash_tool=tool,conditional=True)
                    self.assertNotEqual(result.returncode,0,'conditional accepted '+case)
                    self.assertFalse(marker.exists())


if __name__=='__main__':
    import argparse
    parser=argparse.ArgumentParser();parser.add_argument('--script',type=Path)
    SCRIPT=parser.parse_args().script
    unittest.main(argv=[sys.argv[0]])
