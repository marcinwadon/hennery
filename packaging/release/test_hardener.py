import importlib.util
from pathlib import Path
import unittest

from installer_fixtures import ConsumerFixtures, ROOT, REFUSALS
from test_bootstrap import documented_script

spec=importlib.util.spec_from_file_location('hardener',ROOT/'packaging/harden-installer.py')
hardener=importlib.util.module_from_spec(spec);spec.loader.exec_module(hardener)


class Hardener(ConsumerFixtures,unittest.TestCase):
    def script(self):
        return '''#!/bin/sh
set -eu
check_cmd() { command -v "$1" >/dev/null; }
err() { echo "$*" >&2; exit 1; }
run() {
_dir="$TEST_ROOT/install-work"
_file="$FIXTURE_RELEASE/hennery-test.tar.xz"
_artifact_name=hennery-test.tar.xz
APP_VERSION=0.1.0-alpha.1
'''+hardener.REPLACEMENTS[0][1]+'\nprintf executed > "$INSTALL_MARKER"\n}\nif run; then exit 0; else exit 1; fi\n'

    def test_same_verifier_is_embedded_in_readme_and_installer(self):
        self.assertIn(hardener.CONSUMER_FUNCTIONS,documented_script())
        result=hardener.harden('\n'.join(old for old,_ in hardener.REPLACEMENTS))
        for removed in ('release-inventory','--signer-workflow','release-publish.yml'):
            self.assertNotIn(removed,result)
        for required in ('verify-tag','SHA256SUMS','--source-digest','--signer-digest'):
            self.assertIn(required,result)

    def test_signed_source_and_digest_verification_precedes_extraction(self):
        for case in ('valid',)+REFUSALS:
            with self.subTest(case=case):
                result,marker=self.fixture(case,self.script(),installer=True)
                if case=='valid':
                    self.assertEqual(result.returncode,0,result.stdout+result.stderr)
                    self.assertEqual(marker.read_text(),'executed')
                else:
                    self.assertNotEqual(result.returncode,0,'accepted '+case)
                    self.assertFalse(marker.exists(),'authenticated '+case)

    def test_all_anchors_are_required_once(self):
        pristine='\n'.join(old for old,_ in hardener.REPLACEMENTS)
        result=hardener.harden(pristine)
        self.assertIn('ln "$_stage/$_bin_name" "$_install_dir/"',result)
        self.assertIn('[ "${#_checksum_value}" = 64 ]',result)
        for old,_ in hardener.REPLACEMENTS:
            with self.subTest(anchor=old[:60]):
                with self.assertRaises(SystemExit):hardener.harden(pristine.replace(old,'',1))
                with self.assertRaises(SystemExit):hardener.harden(pristine+'\n'+old)


if __name__=='__main__':unittest.main()
