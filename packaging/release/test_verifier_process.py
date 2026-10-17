"""Real stand-in executables prove the verifier process authority boundary."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

import channel_publish
import contract
import gate
import tag_signature
import test_contract
import verify


class VerifierChildren(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(prefix='verifier-child-test-');self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name);self.tools=self.root/'tools';self.tools.mkdir();self.log=self.root/'children.jsonl'
        self.dirty=self.root/'operator-config';self.dirty.mkdir()
        (self.dirty/'hosts.yml').write_text('credential-canary')
        for filename in ['config.json','config','credentials','.git-credentials','.gitconfig']:(self.dirty/filename).write_text('credential-canary')
        self.bad={'GH_TOKEN':'write-canary','GITHUB_TOKEN':'write-canary','GH_ENTERPRISE_TOKEN':'write-canary',
            'GITHUB_ENTERPRISE_TOKEN':'write-canary','ACTIONS_ID_TOKEN_REQUEST_TOKEN':'oidc-canary',
            'ACTIONS_ID_TOKEN_REQUEST_URL':'https://example.invalid/oidc','ACTIONS_RUNTIME_TOKEN':'runtime-canary',
            'SSH_AUTH_SOCK':'agent-canary','GPG_AGENT_INFO':'agent-canary','GIT_ASKPASS':'helper-canary',
            'SSH_ASKPASS':'helper-canary','GIT_CONFIG_PARAMETERS':'credential.helper=canary',
            'GIT_CONFIG_COUNT':'1','GIT_CONFIG_KEY_0':'http.extraheader','GIT_CONFIG_VALUE_0':'canary',
            'DOCKER_AUTH_CONFIG':'credential-canary','UNKNOWN_FUTURE_SECRET':'canary',
            **{k:'credential-canary' for k in channel_publish.SECRETS},
            **{k:str(self.dirty) for k in ['HOME','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_DATA_HOME','GH_CONFIG_DIR',
                'GNUPGHOME','DOCKER_CONFIG','REGISTRY_AUTH_FILE','ORAS_CONFIG','ORAS_CONFIG_HOME','GIT_CONFIG_GLOBAL','GIT_CONFIG_SYSTEM']},
            'PATH':str(self.tools)+os.pathsep+os.environ['PATH']}
        script='#!'+sys.executable+'\n'+f'''import json,os,sys
from pathlib import Path
name=Path(sys.argv[0]).name
with open({str(self.log)!r},'a') as out:out.write(json.dumps({{'tool':name,'args':sys.argv[1:],'env':dict(os.environ)}})+'\\n')
if name=='gh': print('verified')
elif 'rev-parse' in sys.argv: print({'b'*40!r})
elif 'cat-file' in sys.argv: print({'object '+'a'*40+chr(10)+'type commit'+chr(10)+'tag v0.1.0'+chr(10)+'tagger Test'+chr(10)+chr(10)+'message'+chr(10)!r})
elif 'verify-tag' in sys.argv: print({'[GNUPG:] VALIDSIG '+tag_signature.FINGERPRINT+' 2026 1 0'+chr(10)!r},file=sys.stderr)
'''
        for name in ['gh','git','gpg']:
            path=self.tools/name;path.write_text(script);path.chmod(0o755)

    def children(self):return [json.loads(line) for line in self.log.read_text().splitlines()]

    def assert_clean(self):
        rows=self.children();self.assertTrue(rows)
        allowed_inputs={'PATH','SSL_CERT_FILE','NIX_SSL_CERT_FILE','SSL_CERT_DIR'}
        # Nix's Python launcher injects these two noncredential runtime values
        # after exec; they are not inherited by the subprocess environment.
        runtime={'PYTHONNOUSERSITE','__CF_USER_TEXT_ENCODING'}
        safe_config={'HOME','TMPDIR','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_DATA_HOME','XDG_STATE_HOME',
                     'GH_CONFIG_DIR','GNUPGHOME','DOCKER_CONFIG','REGISTRY_AUTH_FILE','ORAS_CONFIG','ORAS_CONFIG_HOME',
                     'GIT_CONFIG_GLOBAL','GIT_CONFIG_SYSTEM','GIT_CONFIG_NOSYSTEM','GIT_TERMINAL_PROMPT','GIT_NO_REPLACE_OBJECTS',
                     'LC_ALL'}
        for row in rows:
            env=row['env']
            self.assertLessEqual(set(env),allowed_inputs|safe_config|runtime,set(env))
            if 'PYTHONNOUSERSITE' in env:self.assertEqual(env['PYTHONNOUSERSITE'],'true')
            if '__CF_USER_TEXT_ENCODING' in env:self.assertRegex(env['__CF_USER_TEXT_ENCODING'],r'^0x[0-9A-Fa-f]+:0x0:0x0$')
            for key,value in self.bad.items():
                if key=='PATH':continue
                self.assertNotEqual(env.get(key),value,(row['tool'],key))
            self.assertTrue(all(not key.startswith('ACTIONS_') for key in env))
            self.assertNotIn('UNKNOWN_FUTURE_SECRET',env)
            self.assertNotEqual(env['HOME'],str(self.dirty))
            self.assertNotEqual(env['GH_CONFIG_DIR'],str(self.dirty))
            self.assertNotEqual(env['DOCKER_CONFIG'],str(self.dirty))
        for key,value in self.bad.items():self.assertEqual(os.environ.get(key),value,'parent environment mutated')

    def test_release_crypto_children_receive_only_explicit_credential_free_environment(self):
        payload=self.root/'payload';payload.mkdir();test_contract.Contract().fixture(payload)
        with patch.dict(os.environ,self.bad),patch.object(tag_signature,'verify'):
            contract.verify(payload,'v0.1.0','a'*40,self.root/'bundle')
            self.assertEqual(len(self.children()),3);self.assert_clean()

    def test_standalone_image_consumer_child_is_also_credential_free(self):
        manifest=self.root/'manifest';manifest.write_bytes(b'OCI manifest')
        value={'tag':'v0.1.0','source_sha':'a'*40}
        with patch.dict(os.environ,self.bad):
            verify.verify_image_subject('ghcr.io/'+gate.REPOSITORY+'@sha256:'+gate.digest(manifest.read_bytes()),value,
                                       self.root/'bundle',manifest=manifest)
            self.assert_clean()

    def test_git_and_gpg_children_have_no_job_tokens_or_operator_configuration(self):
        with patch.dict(os.environ,self.bad):
            proof=tag_signature.verify('v0.1.0','a'*40,'b'*40)
            self.assertEqual(proof['fingerprint'],tag_signature.FINGERPRINT);self.assert_clean()
            self.assertFalse(any('origin' in row['args'] for row in self.children()),'tag fetch must use the fixed public repository')
            git=[row['args'] for row in self.children() if row['tool']=='git']
            self.assertEqual(sum('init' in args for args in git),1)
            for args in git:
                if 'init' in args:
                    self.assertIn('--bare',args);self.assertIn('--template=',args)
                else:self.assertTrue(any(arg.startswith('--git-dir=/tmp/hennery-verify-') for arg in args))

    def test_parent_api_keeps_token_while_its_real_verifier_child_has_none(self):
        payload=self.root/'payload';payload.mkdir();test_contract.Contract().fixture(payload)
        def parent(*args):
            self.assertEqual(os.environ['GH_TOKEN'],'write-canary')
            contract.verify(payload,'v0.1.0','a'*40,self.root/'bundle')
        with patch.dict(os.environ,self.bad),patch.object(tag_signature,'verify'),patch('release_run.verify',side_effect=parent):
            channel_publish.shared_verify(payload,self.root,'github');self.assert_clean()

    def test_folded_image_verifier_children_are_credential_free(self):
        work=self.root/'work';blobs=work/'layout/blobs/sha256';blobs.mkdir(parents=True)
        raw=b'OCI manifest';digest=gate.digest(raw);(blobs/digest).write_bytes(raw)
        expected={'digest':'sha256:'+digest,'platforms':{}}
        (self.root/'ghcr-index.json').write_bytes(gate.canonical(expected))
        (work/'verified.json').write_bytes(gate.canonical(expected))
        (self.root/'ghcr-index-provenance.json').write_text('local bundle')
        value={'tag':'v0.1.0','source_sha':'a'*40}
        with patch.dict(os.environ,self.bad),patch.object(channel_publish,'image_helper',return_value=SimpleNamespace(inspect_release=lambda *a:expected)):
            channel_publish.check_images(self.root,self.root,value,work);self.assert_clean()
