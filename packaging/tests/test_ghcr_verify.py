"""Read-only OCI verifier; structural crypto doubles and real payload/layout bytes."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch
from test_ghcr_candidate import ReleaseFixture, contract, gate, release_verify, PACKAGING

spec=importlib.util.spec_from_file_location('ghcr_verify',PACKAGING/'ghcr-verify.py')
helper=importlib.util.module_from_spec(spec);spec.loader.exec_module(helper)


class Bundles(ReleaseFixture,unittest.TestCase):
    def setUp(self):
        super().setUp();producer=self.assembled()
        self.index=producer.inspect_release(self.candidate,self.work,self.inventory)
        self.signatures=self.root/'signatures';self.signatures.mkdir()
        (self.signatures/contract.BUNDLE).write_text('{}')
        for *_,bundle in helper.subjects(self.work,self.signatures):bundle.write_text('{}')
        self.calls=[]

    def transport(self,command,**kwargs):
        self.calls.append(command);return self.crypto(command,**kwargs)

    def check(self,**kwargs):
        return helper.check(self.candidate,self.work,self.signatures,self.revision,'v0.1.0',**kwargs)

    def test_authenticates_all_release_payloads_and_seven_exact_local_oci_subjects(self):
        with patch('tag_signature.verify'),patch.object(helper.verify.subprocess,'run',side_effect=self.transport):self.check()
        self.assertEqual(len(self.calls),len(self.inventory['artifacts'])+1+7)
        self.assertTrue(all(c[:3]==['gh','attestation','verify'] and '--signer-workflow' not in c for c in self.calls))
        self.assertEqual({'sha256:'+Path(c[3]).name for c in self.calls[-7:]},{self.index['digest'],*self.index['platforms'].values()})
        self.assertTrue(all('--bundle' in c and not str(c[3]).startswith('oci://') for c in self.calls))
        self.assertEqual(sum(c[c.index('--predicate-type')+1]==helper.SPDX for c in self.calls[-7:]),4)

    def test_any_crypto_refusal_is_final_and_verifier_never_writes_to_registry(self):
        for failing_call in (1,len(self.inventory['artifacts'])+2,len(self.inventory['artifacts'])+8):
            self.calls=[]
            def fail(command,**kwargs):
                if len(self.calls)+1==failing_call:raise subprocess.CalledProcessError(97,command)
                return self.transport(command,**kwargs)
            with self.subTest(call=failing_call),patch('tag_signature.verify'),patch.object(helper.verify.subprocess,'run',side_effect=fail),self.assertRaises(subprocess.CalledProcessError):self.check()
            self.assertTrue(all(c[0]=='gh' for c in self.calls))

    def test_exact_remote_subjects_are_read_only_too(self):
        with patch('tag_signature.verify'),patch.object(helper.verify.subprocess,'run',side_effect=self.transport):self.check(remote=True)
        self.assertTrue(all(str(c[3]).startswith('oci://ghcr.io/marcinwadon/hennery@sha256:') for c in self.calls[-7:]))
        self.assertTrue(all(c[0]=='gh' for c in self.calls))

    def test_missing_oci_bundle_refuses_and_final_subset_is_not_silently_skipped(self):
        next(helper.subjects(self.work,self.signatures))[-1].unlink()
        with patch('tag_signature.verify'),patch.object(helper.verify.subprocess,'run',side_effect=self.transport),self.assertRaisesRegex(gate.Refused,'verification bundle'):self.check()
        self.assertEqual(len(self.calls),len(self.inventory['artifacts'])+1)

    def test_local_manifest_must_hash_to_the_exact_release_digest(self):
        manifest=self.work/'layout/blobs/sha256'/self.index['digest'][7:];original=manifest.read_bytes()
        for data in (b'{}',original+b' '):
            manifest.write_bytes(data);self.calls=[]
            with self.subTest(data=data[:8]),patch('tag_signature.verify'),patch.object(helper.verify.subprocess,'run',side_effect=self.transport),self.assertRaisesRegex(gate.Refused,'local manifest'):self.check()
            self.assertFalse(any(c[3]==str(manifest) for c in self.calls))

    def test_working_metadata_cannot_omit_platform_verification(self):
        path=self.work/'verified.json';changed=json.loads(path.read_bytes());changed['platforms']={};path.write_text(json.dumps(changed))
        with patch('tag_signature.verify'),patch.object(helper.verify.subprocess,'run',side_effect=self.transport),self.assertRaisesRegex(gate.Refused,'release descriptor'):self.check()

    def test_source_tag_or_changed_bytes_refuse(self):
        for source,tag in [('b'*40,'v0.1.0'),(self.revision,'v9.9.9')]:
            with self.subTest(tag=tag),patch.object(helper.verify.subprocess,'run') as crypto,self.assertRaises(gate.Refused):helper.check(self.candidate,self.work,self.signatures,source,tag)
            crypto.assert_not_called()
        def changed(command,**kwargs):
            result=self.transport(command,**kwargs)
            if len(self.calls)==len(self.inventory['artifacts'])+1+7:
                path=self.candidate/'ghcr-index.json';path.write_bytes(path.read_bytes()+b' ')
            return result
        with patch('tag_signature.verify'),patch.object(helper.verify.subprocess,'run',side_effect=changed),self.assertRaises(gate.Refused):self.check()


if __name__=='__main__':unittest.main()
