"""Offline consumer fixtures: genuine disposable OpenPGP tags, mocked provenance."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import shlex
import subprocess
import sys
import tempfile

FINGERPRINT = 'F26C7F594DFE84D131703AFC23E5A318AE8D2861'
ROOT = Path(__file__).resolve().parents[2]


def checked_fixture(command, **kwargs):
    """Keep setup failures diagnosable without dumping the environment."""
    try:
        return subprocess.run(command, check=True, capture_output=True, text=True, **kwargs)
    except subprocess.CalledProcessError as error:
        raise AssertionError('fixture command failed (exit ' + str(error.returncode) + '): '
                             + shlex.join(command) + '\n' + (error.stderr or '(no stderr)')) from None


class ConsumerFixtures:
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix='release-consumer-')
        cls.root = Path(cls.temp.name)
        cls.keyring = cls.root / 'signing'; cls.keyring.mkdir(mode=0o700)
        cls.environment = {key: os.environ[key] for key in
                           ('PATH', 'SSL_CERT_FILE', 'SSL_CERT_DIR', 'NIX_SSL_CERT_FILE') if key in os.environ}
        for key, directory in [('HOME', 'home'), ('XDG_CONFIG_HOME', 'config'),
                               ('XDG_CACHE_HOME', 'cache'), ('XDG_DATA_HOME', 'data')]:
            path = cls.root / directory; path.mkdir(mode=0o700)
            cls.environment[key] = str(path)
        cls.environment.update(GNUPGHOME=str(cls.keyring), TMPDIR=str(cls.root),
                               GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL='/dev/null',
                               GIT_CONFIG_SYSTEM='/dev/null', LC_ALL='C.UTF-8')
        cls.keys, cls.primary = [], []
        for name in ('approved', 'another'):
            cls.gpg('--quick-generate-key', 'Disposable consumer ' + name, 'ed25519', 'cert', '1d')
            listing = cls.gpg('--with-colons', '--list-keys', 'Disposable consumer ' + name).stdout
            primary = next(line.split(':')[9] for line in listing.splitlines() if line.startswith('fpr:'))
            cls.primary.append(primary)
            cls.gpg('--quick-add-key', primary, 'ed25519', 'sign', '1d')
            listing = cls.gpg('--with-colons', '--with-subkey-fingerprint', '--list-keys', primary).stdout
            cls.keys.append([line.split(':')[9] for line in listing.splitlines() if line.startswith('fpr:')][-1])
        cls.public = cls.gpg('--armor', '--export', cls.primary[0]).stdout
        cls.other_public = cls.gpg('--armor', '--export', cls.primary[1]).stdout

    @classmethod
    def tearDownClass(cls):
        checked_fixture(['gpgconf', '--kill', 'gpg-agent'], env=cls.environment)
        cls.temp.cleanup()

    @classmethod
    def gpg(cls, *arguments):
        return checked_fixture(['gpg', '--batch', '--pinentry-mode', 'loopback', '--passphrase', '', *arguments], env=cls.environment)

    def fixture(self, case, script, *, hash_tool='sha256sum', conditional=False, installer=False, tag='v0.1.0-alpha.1'):
        root = Path(tempfile.mkdtemp(prefix='case-', dir=self.root))
        release = root / 'release'; release.mkdir()
        source = root / 'source'
        def git(*args, **kwargs):
            return checked_fixture(['git', '-c', 'user.name=Disposable fixture',
                                    '-c', 'user.email=fixture@example.invalid', *args],
                                   env=self.environment, **kwargs)
        git('init', '--template=', str(source))
        git('-C', str(source), '-c', 'commit.gpgsign=false', 'commit', '--allow-empty', '-m', 'fixture')
        sha = git('-C', str(source), 'rev-parse', 'HEAD').stdout.strip()
        if case == 'lightweight-tag':
            git('-C', str(source), '-c', 'tag.gpgsign=false', 'tag', tag)
        elif case == 'unsigned-tag':
            git('-C', str(source), '-c', 'tag.gpgsign=false', 'tag', '-a', tag, '-m', 'unsigned')
        else:
            actual_tag = 'v9.9.9' if case == 'wrong-release-tag' else tag
            key = self.keys[1 if case == 'wrong-key-signature' else 0]
            git('-C', str(source), '-c', 'gpg.program=gpg', 'tag', '-s', '-u', key+'!', actual_tag, '-m', 'signed fixture')
            if actual_tag != tag:
                git('-C', str(source), 'update-ref', 'refs/tags/'+tag, git('-C', str(source), 'rev-parse', actual_tag).stdout.strip())
        object_sha = git('-C', str(source), 'rev-parse', tag).stdout.strip()
        if case == 'malformed-signature':
            raw = git('-C', str(source), 'cat-file', 'tag', object_sha).stdout.replace('signed fixture', 'changed fixture')
            result = git('-C', str(source), 'hash-object', '-t', 'tag', '-w', '--stdin', input=raw)
            object_sha = result.stdout.strip()
            git('-C', str(source), 'update-ref', 'refs/tags/'+tag, object_sha)
        metadata = {'schema': 3, 'repository': 'marcinwadon/hennery', 'tag': tag, 'source_sha': sha, 'tag_object': object_sha}
        if case == 'wrong-metadata-source': metadata['source_sha'] = '0'*40
        if case == 'wrong-metadata-tag': metadata['tag'] = 'v9.9.9'
        if case == 'wrong-metadata-object': metadata['tag_object'] = '0'*40
        if case == 'wrong-metadata-repository': metadata['repository'] = 'another/repository'
        if case == 'wrong-metadata-schema': metadata['schema'] = 1
        payload_name = 'hennery-test.tar.xz' if installer else 'hennery-installer.sh'
        payload = b'archive fixture' if installer else b'#!/bin/sh\nprintf executed > "$INSTALL_MARKER"\n'
        (release / payload_name).write_bytes(payload)
        (release / 'release-metadata.json').write_text(json.dumps(metadata))
        checksums = ''.join(hashlib.sha256(p.read_bytes()).hexdigest()+'  '+p.name+'\n' for p in sorted(release.iterdir()))
        if case == 'duplicate-checksum': checksums += checksums.splitlines()[0]+'\n'
        if case == 'malformed-checksum': checksums = checksums.replace('  ', ' ', 1)
        if case == 'missing-payload-checksum': checksums = ''.join(line+'\n' for line in checksums.splitlines() if not line.endswith(payload_name))
        if case == 'wrong-payload-checksum': checksums = checksums.replace(hashlib.sha256(payload).hexdigest(), '0'*64)
        if case == 'unsafe-checksum': checksums += '0'*64+'  ../escape\n'
        if case == 'cyclic-checksum': checksums += '0'*64+'  SHA256SUMS\n'
        if case == 'no-final-newline': checksums = checksums.rstrip('\n')
        (release / 'SHA256SUMS').write_text(checksums)
        (release / 'release.sigstore.jsonl').write_text('offline provenance fixture\n')
        attested = root/'attested'; shutil.copytree(release, attested)
        if case == 'tampered-installer': (release/payload_name).write_bytes(payload+b'tampered')
        if case == 'tampered-checksums': (release/'SHA256SUMS').write_text(checksums+' ')
        if case == 'tampered-metadata': (release/'release-metadata.json').write_text('{}')
        if case == 'missing-bundle': (release/'release.sigstore.jsonl').unlink()
        if case == 'missing-checksums': (release/'SHA256SUMS').unlink()
        if case == 'missing-metadata': (release/'release-metadata.json').unlink()
        (release/'github.asc').write_text(self.other_public if case == 'wrong-github-key' else self.public+self.other_public)
        (release/'keyserver.asc').write_text(self.other_public if case == 'wrong-keyserver-key' else self.public)
        (release/'main-readme').write_text('Fingerprint: '+('0'*40 if case == 'wrong-main-pin' else self.keys[0])+'\n')
        tools = root/'tools'; tools.mkdir()
        real_git = shutil.which('git')
        (tools/'git').write_text('#!'+sys.executable+'\n'+f'''import os,sys,subprocess
args=sys.argv[1:]
url='https://github.com/marcinwadon/hennery.git'
if 'fetch' in args:
    assert args[-2:]==[url,'refs/tags/'+os.environ['FIXTURE_TAG']]
    args[args.index(url)]=os.environ['FIXTURE_SOURCE']
if 'verify-tag' in args and os.environ['FIXTURE_CASE'] in ('expired-status','revoked-status','duplicate-status','empty-status','failed-valid-status'):
    result=subprocess.run([{real_git!r},*args],capture_output=True)
    assert result.returncode==0
    case=os.environ['FIXTURE_CASE']
    status=result.stderr
    if case=='expired-status': status+=b'[GNUPG:] KEYEXPIRED 123\\n'
    if case=='revoked-status': status+=b'[GNUPG:] REVKEYSIG bad\\n'
    if case=='duplicate-status': status+=status
    if case=='empty-status': status=b''
    sys.stderr.buffer.write(status)
    sys.exit(1 if case=='failed-valid-status' else 0)
os.execv({real_git!r},[{real_git!r},*args])
''')
        (tools/'curl').write_text('#!'+sys.executable+'\n'+'''import os,sys
from pathlib import Path
args=sys.argv[1:]
assert args[args.index('--proto')+1]=='=https'
assert args[args.index('--proto-redir')+1]=='=https'
url=next(arg for arg in args if arg.startswith('https://'))
base='https://github.com/marcinwadon/hennery/releases/download/'+os.environ['FIXTURE_TAG']+'/'
names={'https://raw.githubusercontent.com/marcinwadon/hennery/refs/heads/main/README.md':'main-readme',
       'https://github.com/marcinwadon.gpg':'github.asc',
       'https://keys.openpgp.org/vks/v1/by-fingerprint/'+os.environ['FIXTURE_FINGERPRINT']:'keyserver.asc'}
names.update({base+name:name for name in ['SHA256SUMS','release-metadata.json','release.sigstore.jsonl','hennery-installer.sh']})
if url not in names: sys.exit(99)
path=Path(os.environ['FIXTURE_RELEASE'])/names[url]
if not path.is_file(): sys.exit(22)
Path(args[args.index('-o')+1]).write_bytes(path.read_bytes())
''')
        (tools/'gh').write_text('#!'+sys.executable+'\n'+'''import json,os,sys
from pathlib import Path
a=sys.argv[1:]; assert a[:2]==['attestation','verify']
assert '--signer-workflow' not in a
for flag,expected in {'--repo':'marcinwadon/hennery','--source-ref':'refs/tags/'+os.environ['FIXTURE_TAG'],'--source-digest':os.environ['FIXTURE_SHA'],'--signer-digest':os.environ['FIXTURE_SHA'],'--cert-identity':'https://github.com/marcinwadon/hennery/.github/workflows/release.yml@refs/tags/'+os.environ['FIXTURE_TAG'],'--cert-oidc-issuer':'https://token.actions.githubusercontent.com','--predicate-type':'https://slsa.dev/provenance/v1'}.items():
    assert a.count(flag)==1 and a[a.index(flag)+1]==expected
assert '--deny-self-hosted-runners' in a
subject=Path(a[2]); bundle=Path(a[a.index('--bundle')+1]); assert bundle.read_text()=='offline provenance fixture\\n'
assert subject.read_bytes()==(Path(os.environ['FIXTURE_ATTESTED'])/subject.name).read_bytes()
case=os.environ['FIXTURE_CASE']
if case=='branch-signer': sys.exit(1)
print('verified')
if case=='failed-attestation-status': sys.exit(1)
''')
        for tool in tools.iterdir(): tool.chmod(0o755)
        for name in ('gpg','cat','tail','env','awk','grep','jq','mktemp','mkdir','chmod','sh',hash_tool):
            executable=shutil.which(name); self.assertIsNotNone(executable,name)
            (tools/name).symlink_to(executable)
        missing = {'missing-gh':'gh','missing-git':'git','missing-gpg':'gpg','missing-jq':'jq','missing-hash':hash_tool}
        if case in missing: (tools/missing[case]).unlink()
        if case == 'failed-hash-status':
            executable=shutil.which(hash_tool)
            (tools/hash_tool).unlink()
            (tools/hash_tool).write_text('#!'+sys.executable+'\n'+f'import subprocess,sys\nsubprocess.run([{executable!r},*sys.argv[1:]],check=True)\nsys.exit(1)\n')
            (tools/hash_tool).chmod(0o755)
        pin='release_fingerprint='+FINGERPRINT
        self.assertEqual(script.count(pin),1)
        script=script.replace(pin,'release_fingerprint='+self.keys[0])
        if installer:
            import importlib.util
            spec=importlib.util.spec_from_file_location('hardener_fixture',ROOT/'packaging/harden-installer.py')
            hardener=importlib.util.module_from_spec(spec);spec.loader.exec_module(hardener)
            script=script.replace(hardener.MAINTAINER_KEY,self.public)
        if conditional: script='if '+script+'then exit 0; else exit 1; fi\n'
        path=root/'check.sh';path.write_text(script)
        home=root/'home';home.mkdir()
        marker=root/'executed'
        env=dict(self.environment, PATH=str(tools),HOME=str(home),GNUPGHOME=str(home/'unused'),TMPDIR=str(self.root),INSTALL_MARKER=str(marker),FIXTURE_RELEASE=str(release),FIXTURE_FINGERPRINT=self.keys[0],FIXTURE_SOURCE=str(source),FIXTURE_SHA=sha,FIXTURE_ATTESTED=str(attested),FIXTURE_CASE=case,TEST_ROOT=str(root),HENNERY_RELEASE_CHECKSUMS=str(release/'SHA256SUMS'),HENNERY_RELEASE_METADATA=str(release/'release-metadata.json'),HENNERY_ATTESTATION_BUNDLE=str(release/'release.sigstore.jsonl'))
        env['FIXTURE_TAG']=tag
        (root/'install-work').mkdir()
        result=subprocess.run([str(tools/'sh'),str(path)],env=env,cwd=root,capture_output=True,text=True)
        return result,marker


REFUSALS = ('unsigned-tag','lightweight-tag','malformed-signature','wrong-key-signature','wrong-release-tag',
            'tampered-installer','tampered-checksums','tampered-metadata','missing-bundle','missing-checksums','missing-metadata',
            'wrong-metadata-source','wrong-metadata-tag','wrong-metadata-object','wrong-metadata-repository','wrong-metadata-schema',
            'duplicate-checksum','malformed-checksum','missing-payload-checksum','wrong-payload-checksum',
            'unsafe-checksum','cyclic-checksum','branch-signer',

            'expired-status','revoked-status','duplicate-status','empty-status','failed-valid-status',
            'failed-attestation-status','failed-hash-status','missing-gh','missing-git','missing-gpg','missing-jq','missing-hash','no-final-newline')
