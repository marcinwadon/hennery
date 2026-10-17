"""New release contract binds real R4 OCI fixtures; crypto is covered separately."""
from pathlib import Path
import sys
import unittest
import subprocess
from unittest.mock import patch

from test_ghcr_candidate import ReleaseFixture,load_helper,TARGETS
sys.path.insert(0,str(Path(__file__).parents[1]/'release'))
import contract
import gate
import supply
import channel_publish


class ReleaseImages(ReleaseFixture,unittest.TestCase):
    def setUp(self):
        super().setUp();self.helper=load_helper(self)
        self.helper.assemble(self.candidate,self.root/'assembled',self.natives,self.revision,self.version)
        value={'schema':3,'repository':gate.REPOSITORY,'source_sha':self.revision,'tag':'v'+self.version,'tag_object':'b'*40,
               'channels':['ghcr','github'],'targets':sorted(TARGETS.values()),
               'binaries':supply.binary_inventory(self.candidate,sorted(TARGETS.values()))}
        contract.freeze(self.candidate,value)

    def test_materializes_exact_checksum_bound_oci_without_inventory_signature(self):
        self.assertTrue(hasattr(self.helper,'inspect_release'))
        value=self.helper.inspect_release(self.candidate,self.work,contract.load(self.candidate))
        self.assertEqual(value,gate.strict_json((self.candidate/'ghcr-index.json').read_bytes()))
        self.assertFalse((self.work/'release-inventory.json.asc').exists())

    def test_tampered_oci_or_working_layout_refuses(self):
        self.assertTrue(hasattr(self.helper,'inspect_release'))
        self.helper.inspect_release(self.candidate,self.work,contract.load(self.candidate))
        path=self.work/'layout/oci-layout';path.write_text('{}')
        with self.assertRaises(gate.Refused):self.helper.inspect_release(self.candidate,self.work,contract.load(self.candidate))
        path=self.candidate/'ghcr-oci-layout.tar';path.write_bytes(path.read_bytes()+b'x')
        with self.assertRaises(gate.Refused):contract.load(self.candidate)

    def test_seven_exact_local_oci_predicates_verify_before_registry_credentials(self):
        value=contract.load(self.candidate);signatures=self.root/'signatures';signatures.mkdir()
        self.helper.inspect_release(self.candidate,self.work,value)
        for *_,bundle in channel_publish.image_subjects(self.work,signatures):bundle.write_text('{}')
        calls=[]
        def crypto(command,**kwargs):
            calls.append(command)
            return subprocess.CompletedProcess(command,0,'verified','')
        with patch.object(channel_publish.subprocess,'run',side_effect=crypto):
            channel_publish.check_images(self.candidate,signatures,value,self.work)
        self.assertEqual(len(calls),7)
        self.assertTrue(all(c[0]=='gh' and not str(c[3]).startswith('oci://') and '--signer-workflow' not in c for c in calls))
        self.assertEqual(sum(c[c.index('--predicate-type')+1]=='https://spdx.dev/Document' for c in calls),4)
        next(channel_publish.image_subjects(self.work,signatures))[-1].unlink()
        with patch.object(channel_publish.subprocess,'run',side_effect=crypto),self.assertRaises(gate.Refused):
            channel_publish.check_images(self.candidate,signatures,value,self.work)

    def test_layer_change_between_verification_and_write_refuses(self):
        value=contract.load(self.candidate)
        self.helper.inspect_release(self.candidate,self.work,value)
        signatures=self.root/'signatures';signatures.mkdir()
        for *_,bundle in channel_publish.image_subjects(self.work,signatures):bundle.write_text('{}')
        layer=next(p for p in (self.work/'layout/blobs/sha256').iterdir() if p.read_bytes().startswith(b'\x1f\x8b'))
        layer.write_bytes(b'changed layer')
        with patch.object(channel_publish.subprocess,'run',side_effect=AssertionError('crypto or registry reached')),self.assertRaises((gate.Refused,ValueError)):
            channel_publish.check_images(self.candidate,signatures,value,self.work)
