"""Folded GHCR lifecycle with standard attachment transport and no registry access.

Real OCI closure lives in test_release_images; actual HTTP status classification
lives in test_ghcr_remote; authentic crypto lives in test_verify_cli.
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
sys.path.insert(0,str(Path(__file__).parents[1]/'release'))
import channel_publish as publisher
import gate


class GHCR(unittest.TestCase):
    def execute(self, mode='new', boundary=0):
        self.events=[];self.copies=0;self.attaches=0;present=mode in ('present','partial');reconciles=0;checks=0
        digest='sha256:'+'a'*64
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);value={'tag':'v0.1.0'}
            def verify(*args,**kwargs):
                nonlocal checks
                checks+=1;self.events.append('verify')
                if checks==boundary:raise gate.Refused('verification withdrawn')
                if kwargs.get('subjects'):
                    self.assertTrue(all(item[4].parent.name=='fetched' for item in kwargs['subjects']))
                if kwargs.get('subjects') and mode=='remote-failure':raise gate.Refused('fetched signature refused')
            def shared(*args):
                self.events.append('shared')
                if mode=='approval-withdrawn' and self.copies==0 and self.events.count('shared')>1:raise gate.Refused('review withdrawn')
                if mode=='referrer-withdrawn' and self.copies>0:raise gate.Refused('review withdrawn before referrer')
                return value
            def reconcile(*args):
                nonlocal reconciles
                reconciles+=1;self.events.append('reconcile')
                if mode in ('conflict','unauthorized','timeout'):raise gate.Refused('remote refused')
                if mode=='late-conflict' and reconciles>=3:raise gate.Refused('new conflicting version')
                return 0 if present else 1
            def run(command,**kwargs):
                nonlocal present
                operation=command[0]+' '+command[1];self.events.append(operation)
                if operation=='skopeo login':
                    self.assertIn('verify',self.events[:-1]);self.assertEqual(kwargs['input'],'offline-credential')
                    self.assertNotIn('offline-credential',str(command))
                    Path(command[command.index('--authfile')+1]).write_text('{}')
                if operation=='skopeo copy':
                    self.copies+=1;self.assertIn('--preserve-digests',command);self.assertIn('--all',command)
                    self.assertEqual(command[-1],'docker://ghcr.io/marcinwadon/hennery@'+digest)
                    self.assertNotIn(':latest',' '.join(command));self.assertNotIn(':0.1.0',' '.join(command));present=True
                    if mode=='copy-failure':raise subprocess.CalledProcessError(1,command)
            def attach(digest,predicate,bundle,auth,environment):
                self.assertNotIn('GH_TOKEN',environment)
                self.assertNotIn('ACTIONS_ID_TOKEN_REQUEST_TOKEN',environment)
                self.attaches+=1
                if mode=='attach-failure':raise subprocess.CalledProcessError(1,['oras','attach'])
            def fetched(*args,**kwargs):
                self.events.append('fetched-verify')
                if mode=='remote-failure':raise gate.Refused('fetched signature refused')
            subjects=[('index','provenance',digest,'slsa',root/'index.json')]+[(arch,kind,digest,'slsa' if kind=='provenance' else 'spdx',root/(arch+kind+'.json')) for arch in ('amd64','arm64') for kind in ('provenance','image-sbom','binary-sbom')]
            with patch.dict(os.environ,{'GHCR_PUBLISH_TOKEN':'offline-credential','GHCR_PUBLISH_USER':'fixture','GH_TOKEN':'write-canary','ACTIONS_ID_TOKEN_REQUEST_TOKEN':'oidc-canary'}),patch.object(publisher,'shared_verify',side_effect=shared),patch.object(publisher,'image_helper',return_value=SimpleNamespace(inspect_release=lambda *a:{'digest':digest})),patch.object(publisher,'registry_helper',return_value=SimpleNamespace(reconcile=reconcile)),patch.object(publisher,'check_images',side_effect=verify),patch.object(publisher,'image_subjects',return_value=subjects),patch.object(publisher,'attach_bundle',side_effect=attach),patch.object(publisher.verify,'verify_image_subject',side_effect=fetched),patch.object(publisher.subprocess,'run',side_effect=run):
                return publisher.publish(root,root,'ghcr')

    def test_absent_uploads_only_digest_and_all_seven_bundles(self):
        self.execute();self.assertEqual(self.copies,1);self.assertEqual(self.attaches,7)
    def test_matching_retry_reuses_image_and_allows_duplicate_bundles(self):
        self.execute('present');self.assertEqual(self.copies,0);self.assertEqual(self.attaches,7)
    def test_conflict_auth_timeout_and_late_version_never_overwrite(self):
        for mode in ('conflict','unauthorized','timeout','late-conflict'):
            with self.subTest(mode=mode),self.assertRaises(gate.Refused):self.execute(mode)
            self.assertEqual(self.copies,0);self.assertEqual(self.attaches,0)
    def test_ambiguous_upload_stops_then_new_approved_retry_reconciles(self):
        with self.assertRaises(subprocess.CalledProcessError):self.execute('copy-failure')
        self.assertEqual(self.copies,1);self.assertEqual(self.attaches,0)
        self.execute('present');self.assertEqual(self.copies,0)
    def test_partial_bundle_upload_stops_then_matching_retry_reattaches_valid_bundles(self):
        with self.assertRaises(subprocess.CalledProcessError):self.execute('attach-failure')
        self.assertEqual(self.attaches,1)
        self.execute('partial');self.assertEqual(self.copies,0);self.assertEqual(self.attaches,7)
        self.execute('present');self.assertEqual(self.copies,0);self.assertEqual(self.attaches,7)
    def test_remote_verification_failure_is_not_channel_success(self):
        with self.assertRaises(gate.Refused):self.execute('remote-failure')
        self.assertEqual(self.attaches,7)
