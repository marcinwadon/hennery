"""Draft, immutable publication and retry lifecycle with a fake HTTP boundary."""
import copy
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import contract
import gate
import publication


class Publication(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name)/'payload';self.root.mkdir()
        self.signatures=Path(self.tmp.name)/'signatures';self.signatures.mkdir()
        (self.root/'installer.sh').write_bytes(b'exit 0\n')
        contract.freeze(self.root,{'schema':3,'repository':gate.REPOSITORY,'source_sha':'a'*40,'tag':'v0.1.0',
            'tag_object':'b'*40,'channels':['github'],'targets':sorted(gate.TARGETS)})
        (self.signatures/contract.BUNDLE).write_bytes(b'authentic bundle doubled here')
        self.remote=None;self.assets={};self.events=[];self.fault=None

    def authorize(self):
        self.events.append('verify')
        if self.fault=='gate' or (self.fault=='later-gate' and self.events.count('verify')>1):raise gate.Refused('gate refused')
        return contract.load(self.root)

    def api(self,path,method='GET',data=None,**kwargs):
        import urllib.parse
        if method=='GET':
            if '/assets?' in path:return [{'name':name,'url':'https://api.github.com/repos/'+gate.REPOSITORY+'/releases/assets/'+name} for name in self.assets]
            if '/releases/assets/' in path:return self.assets[path.rsplit('/',1)[1]]
            if self.fault=='late-state' and self.remote and self.remote['immutable']:
                return dict(self.remote,draft=True,immutable=False)
            return copy.deepcopy(self.remote)
        self.events.append(method)
        self.assertEqual(self.events[-2],'verify','each write requires fresh verification')
        if method=='POST' and '/assets?' not in path:
            self.remote={'id':7,'tag_name':'v0.1.0','draft':True,'immutable':False}
            return copy.deepcopy(self.remote)
        if method=='POST':
            name=urllib.parse.parse_qs(urllib.parse.urlsplit(path).query)['name'][0]
            self.assets[name]=data if self.fault!='readback' else data+b'wrong'
            return {'id':len(self.assets)}
        if method=='PATCH':
            self.remote.update(draft=False,immutable=self.fault!='mutable')
            if self.fault=='ambiguous':raise gate.Refused('response lost after successful publication')
            return copy.deepcopy(self.remote)
        self.fail('unexpected HTTP method')

    def run_publish(self):
        with patch.object(publication.transport,'api',side_effect=self.api),patch.object(publication.transport,'asset_bytes',side_effect=lambda a:self.assets[a['name']]):
            return publication.release(self.root,self.signatures,self.authorize)

    def test_verified_draft_upload_readback_and_immutable_publication(self):
        result=self.run_publish()
        self.assertTrue(self.remote and self.remote['immutable'])
        self.assertEqual(result['state'],'immutable')
        self.assertEqual(set(self.assets),{p.name for p in self.root.iterdir()}|{contract.BUNDLE})

    def test_failed_gate_never_creates_or_uploads(self):
        self.fault='gate'
        with self.assertRaises(gate.Refused):self.run_publish()
        self.assertEqual(self.events,['verify'])

    def test_remote_conflict_or_extra_asset_prevents_any_write(self):
        for name,data in [('installer.sh',b'wrong'),('unapproved',b'x')]:
            self.remote={'id':7,'tag_name':'v0.1.0','draft':True,'immutable':False};self.assets={name:data};self.events=[]
            with self.subTest(name=name),self.assertRaises(gate.Refused):self.run_publish()
            self.assertFalse(any(e in ('POST','PATCH') for e in self.events))

    def test_changed_authority_after_preflight_refuses_the_first_mutation(self):
        self.fault='later-gate'
        with self.assertRaises(gate.Refused):self.run_publish()
        self.assertFalse(any(e in ('POST','PATCH') for e in self.events))

    def test_upload_readback_or_immutability_failure_refuses(self):
        for fault in ['readback','mutable','late-state']:
            self.remote=None;self.assets={};self.events=[];self.fault=fault
            with self.subTest(fault=fault),self.assertRaises(gate.Refused):self.run_publish()

    def test_ambiguous_success_is_reconciled_without_republishing_on_retry(self):
        self.fault='ambiguous'
        with self.assertRaises(gate.Refused):self.run_publish()
        self.assertTrue(self.remote['immutable'])
        self.fault=None;self.events=[]
        self.assertEqual(self.run_publish()['state'],'immutable')
        self.assertFalse(any(e in ('POST','PATCH') for e in self.events))

    def test_incomplete_immutable_release_requires_new_version(self):
        self.remote={'id':7,'tag_name':'v0.1.0','draft':False,'immutable':True}
        with self.assertRaises(gate.Refused):self.run_publish()
        self.assertFalse(any(e in ('POST','PATCH') for e in self.events))
