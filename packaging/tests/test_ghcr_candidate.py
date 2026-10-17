"""Real OCI/archive fixtures; cryptographic transports are explicit structural doubles."""
import copy
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import unittest
from unittest.mock import patch

from test_image import CandidateFixtures, image

PACKAGING = Path(__file__).parents[1]
sys.path.insert(0, str(PACKAGING / 'release'))
import contract
import gate
import supply
import verify as release_verify

TARGETS = {'amd64': 'x86_64-unknown-linux-musl', 'arm64': 'aarch64-unknown-linux-musl'}


def load_helper(test):
    path = PACKAGING / 'ghcr-candidate.py'
    test.assertTrue(path.is_file(), 'release must materialize the checksum-bound OCI layout')
    spec = importlib.util.spec_from_file_location('ghcr_candidate', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ReleaseFixture(CandidateFixtures):
    def setUp(self):
        super().setUp()
        self.version = '0.1.0'
        self.candidate = self.root / 'candidate'
        self.candidate.mkdir()
        self.work = self.root / 'image-candidate'
        self.natives = []
        base = self.root
        for arch, target in TARGETS.items():
            self.root = base / arch
            (self.root / 'blobs/sha256').mkdir(parents=True)
            self.fixture(arch=arch, labels={'org.opencontainers.image.version': self.version})
            self.natives.append(self.root)
            archive = self.candidate / ('hennery-' + target + '.tar.xz')
            prefix = 'hennery-' + target
            with tarfile.open(archive, 'w:xz') as output:
                directory = tarfile.TarInfo(prefix); directory.type = tarfile.DIRTYPE
                output.addfile(directory)
                for name, data in [('hennery', self.binary), ('LICENSE', b'licence'),
                                   ('CHANGELOG.md', b'changelog'), ('README.md', b'readme'), ('THIRD-PARTY-NOTICES.txt', b'notices')]:
                    member = tarfile.TarInfo(prefix + '/' + name); member.size = len(data)
                    output.addfile(member, io.BytesIO(data))
            archive.with_suffix(archive.suffix + '.sha256').write_text(gate.digest(archive.read_bytes()) + '  ' + archive.name + '\n')
        self.root = base


        self.real_run = subprocess.run
        def safe_transport(command, **kwargs):
            if command[:2] == [sys.executable, str(PACKAGING / 'ghcr-candidate.py')]:
                return self.real_run(command, **kwargs)  # local pure-data CLI only
            if command[:3] == ['gh', 'attestation', 'verify']:
                return self.crypto(command, **kwargs)  # structural, never external
            raise AssertionError('unstubbed external transport: ' + repr(command))
        transport = patch.object(subprocess, 'run', side_effect=safe_transport)
        transport.start(); self.addCleanup(transport.stop)
        tag = patch('tag_signature.verify', return_value={})
        tag.start(); self.addCleanup(tag.stop)

    def freeze(self, changes=None):
        for name in (contract.METADATA, contract.CHECKSUMS):
            (self.candidate / name).unlink(missing_ok=True)
        value = {'schema': 3, 'repository': gate.REPOSITORY, 'source_sha': self.revision,
                 'tag': 'v' + self.version, 'tag_object': 'b' * 40, 'channels': ['ghcr','github'],
                 'targets': sorted(TARGETS.values()),
                 'binaries': supply.binary_inventory(self.candidate, sorted(TARGETS.values()))}
        contract.freeze(self.candidate, value | (changes or {}))
        self.inventory = contract.load(self.candidate)

    def crypto(self, command, **kwargs):
        return subprocess.CompletedProcess(command, 0, 'verified', '')

    def assembled(self):
        helper = load_helper(self)
        helper.assemble(self.candidate, self.root / 'assembled', self.natives, self.revision, self.version)
        self.freeze()
        return helper


class Candidate(ReleaseFixture, unittest.TestCase):
    def test_acceptance_binds_exact_source_tag_without_publication_authority(self):
        helper = self.assembled()
        for revision, version in [('b' * 40, self.version), (self.revision, '0.2.0')]:
            with self.subTest(revision=revision, version=version), self.assertRaisesRegex(gate.Refused, 'release source/tag'):
                helper.accept(self.candidate, self.work, revision, version)
        accepted = helper.accept(self.candidate, self.work, self.revision, self.version)
        self.assertEqual(accepted, json.loads((self.candidate / 'ghcr-index.json').read_bytes()))
        result = subprocess.run([sys.executable, str(PACKAGING / 'ghcr-candidate.py'), 'accept',
            str(self.candidate), str(self.root / 'cli-acceptance'), self.revision, self.version], text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), accepted)

    def test_native_output_binds_index_platforms_and_binary_identity(self):
        helper = self.assembled()
        verified = helper.inspect_release(self.candidate, self.work, self.inventory)
        self.assertEqual(verified, json.loads((self.candidate / 'ghcr-index.json').read_bytes()))
        self.assertEqual(set(verified['platforms']), {'amd64','arm64'})
        for arch in TARGETS:
            self.assertEqual(verified['platforms'][arch], image.verify(self.work / 'layout',
                {a:self.inventory['binaries'][t]['sha256'] for a,t in TARGETS.items()},
                self.revision,self.version)['platforms'][arch]['digest'])

    def test_binary_archive_validation_is_required_at_materialization(self):
        helper=self.assembled()
        with patch.object(supply,'verify_binary_inventory',wraps=supply.verify_binary_inventory) as check:
            helper.inspect_release(self.candidate,self.work,self.inventory)
            check.assert_called_once_with(self.inventory,self.candidate)

    def test_missing_changed_or_extra_release_bytes_refuse(self):
        helper = self.assembled()
        path = self.candidate / 'ghcr-index.json'; raw = path.read_bytes()
        for kind in ('changed','missing','extra'):
            if kind == 'changed': path.write_bytes(raw + b' ')
            if kind == 'missing': path.unlink()
            if kind == 'extra': (self.candidate / 'unlisted').write_bytes(b'bytes')
            with self.subTest(kind=kind), self.assertRaises(gate.Refused):
                helper.inspect_release(self.candidate,self.work,self.inventory)
            path.write_bytes(raw); (self.candidate / 'unlisted').unlink(missing_ok=True)

    def test_stale_metadata_and_missing_ghcr_or_linux_target_refuse(self):
        helper = self.assembled()
        stale = copy.deepcopy(self.inventory); stale['source_sha'] = 'b' * 40
        with self.assertRaisesRegex(gate.Refused,'metadata changed'):
            helper.inspect_release(self.candidate,self.work,stale)
        for changes in ({'channels':['github']},{'targets':[TARGETS['amd64']]}):
            self.freeze(changes)
            with self.subTest(changes=changes), self.assertRaisesRegex(gate.Refused,'GHCR and both Linux'):
                helper.inspect_release(self.candidate,self.work,self.inventory)

    def test_binary_identity_and_source_bound_descriptor_must_match_oci(self):
        helper = self.assembled()
        path = self.candidate / 'ghcr-index.json'; original = path.read_bytes()
        for field,value in [('source_sha','c'*40),('version','0.2.0'),('platforms',{'amd64':'sha256:'+'f'*64,'arm64':'sha256:'+'e'*64})]:
            changed=json.loads(original);changed[field]=value;path.write_bytes(gate.canonical(changed));self.freeze()
            with self.subTest(field=field),self.assertRaisesRegex(gate.Refused,'descriptor'):
                helper.inspect_release(self.candidate,self.work,self.inventory)
            path.write_bytes(original)
        self.freeze({'binaries':{}})
        with self.assertRaisesRegex(gate.Refused,'binary'):
            helper.inspect_release(self.candidate,self.work,self.inventory)

    def test_working_directory_is_outside_release_and_never_replaced(self):
        helper = self.assembled()
        with self.assertRaisesRegex(gate.Refused,'outside'):
            helper.inspect_release(self.candidate,self.candidate / 'work',self.inventory)
        helper.inspect_release(self.candidate,self.work,self.inventory)
        path=self.work / 'layout/oci-layout';path.write_text('{}')
        with self.assertRaisesRegex(gate.Refused,'working OCI'):
            helper.inspect_release(self.candidate,self.work,self.inventory)
        self.assertEqual(path.read_text(),'{}')

    def test_working_symlinks_and_metadata_tampering_refuse(self):
        helper=self.assembled();helper.inspect_release(self.candidate,self.work,self.inventory)
        alias=self.root / 'alias';alias.symlink_to(self.work,target_is_directory=True)
        with self.assertRaisesRegex(gate.Refused,'nonregular image working directory'):
            helper.inspect_release(self.candidate,alias,self.inventory)
        path=self.work / 'layout/oci-layout';path.unlink();path.symlink_to(self.natives[0] / 'oci-layout')
        with self.assertRaisesRegex(gate.Refused,'nonregular working OCI'):
            helper.inspect_release(self.candidate,self.work,self.inventory)
        path.unlink();path.write_bytes((self.natives[0] / 'oci-layout').read_bytes())
        (self.work / 'index-digest.txt').write_text('sha256:'+'f'*64)
        with self.assertRaisesRegex(gate.Refused,'metadata changed'):
            helper.inspect_release(self.candidate,self.work,self.inventory)

    def test_payload_changed_after_checksum_verification_refuses(self):
        helper=self.assembled();original=contract.load
        def changed(root):
            result=original(root)
            path=root / 'ghcr-index.json';path.write_bytes(path.read_bytes()+b' ')
            return result
        with patch.object(contract,'load',side_effect=changed),self.assertRaisesRegex(gate.Refused,'during verification'):
            helper.inspect_release(self.candidate,self.work,self.inventory)

    def test_existing_assembly_output_refuses_without_overwriting(self):
        helper = load_helper(self); work = self.root / 'existing'; work.mkdir()
        with self.assertRaisesRegex(gate.Refused, 'already exists'):
            helper.assemble(self.candidate, work, self.natives, self.revision, self.version)
        self.assertFalse((self.candidate / 'ghcr-index.json').exists())


    def test_descriptor_needs_one_retained_root_index(self):
        helper = load_helper(self); layout = self.root / 'flat'
        digest = image.merge(layout, self.natives)
        joined = json.loads((layout / 'blobs/sha256' / digest[7:]).read_bytes())
        (layout / 'index.json').write_text(json.dumps({'schemaVersion': 2, 'manifests': joined['manifests']}))
        with self.assertRaisesRegex(gate.Refused, 'one OCI index'):
            helper.descriptor(layout, dict.fromkeys(TARGETS, self.expected), self.revision, self.version)


    def archive(self, entries):
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode='w') as archive:
            for name, kind, data in entries:
                member = tarfile.TarInfo(name); member.type = kind
                if kind == tarfile.REGTYPE:
                    member.size = len(data); archive.addfile(member, io.BytesIO(data))
                else:
                    member.linkname = data; archive.addfile(member)
        return output.getvalue()


    def test_ambiguous_oci_archive_paths_refuse(self):
        helper = load_helper(self)
        for name in ['../escape', './index.json', '/index.json', 'blobs//sha256/' + 'a' * 64, 'blobs/sha256/../escape']:
            with self.subTest(name=name), self.assertRaisesRegex(gate.Refused, 'archive path'):
                helper.tar_files(self.archive([(name, tarfile.REGTYPE, b'{}')]))


    def test_oci_archive_links_refuse_before_extraction(self):
        helper = load_helper(self)
        for kind in [tarfile.LNKTYPE, tarfile.SYMTYPE]:
            raw = self.archive([('index.json', tarfile.REGTYPE, b'{}'),
                ('blobs/sha256/' + 'a' * 64, kind, 'index.json')])
            with self.subTest(kind=kind), self.assertRaisesRegex(gate.Refused, 'nonregular'):
                helper.tar_files(raw)


    def test_duplicate_oci_archive_members_refuse(self):
        helper = load_helper(self)
        raw = self.archive([('index.json', tarfile.REGTYPE, b'{}'), ('index.json', tarfile.REGTYPE, b'{}')])
        with self.assertRaisesRegex(gate.Refused, 'duplicate'):
            helper.tar_files(raw)



class Consumer(ReleaseFixture, unittest.TestCase):
    def setUp(self):
        super().setUp();self.assembled()
        self.index=gate.strict_json((self.candidate / 'ghcr-index.json').read_bytes())

    def call(self,digest,**kwargs):
        return release_verify.verify_image('ghcr.io/marcinwadon/hennery@'+digest,'v0.1.0',
            release_directory=self.candidate,release_bundle=self.root/'bundle',source_sha=self.revision,
            **kwargs)

    def test_tag_authentication_precedes_sigstore(self):
        with patch('tag_signature.verify',side_effect=gate.Refused('bad tag')),patch.object(release_verify.subprocess,'run') as external:
            with self.assertRaisesRegex(gate.Refused,'bad tag'):self.call(self.index['digest'])
            external.assert_not_called()

    def test_unlisted_or_mutable_image_refuses_before_sigstore(self):
        with patch('tag_signature.verify'),patch.object(release_verify.subprocess,'run',side_effect=self.crypto) as external:
            with self.assertRaisesRegex(gate.Refused,'image digest'):self.call('sha256:'+'f'*64)
            with self.assertRaisesRegex(gate.Refused,'immutable image'):
                release_verify.verify_image('ghcr.io/marcinwadon/hennery:latest','v0.1.0')
            external.assert_not_called()

    def test_descriptor_identity_platform_set_and_distinct_digest_are_required(self):
        path=self.candidate / 'ghcr-index.json'
        for field,value in [('schema',2),('source_sha','b'*40),('version','0.2.0'),
                            ('platforms',['amd64','arm64']),('platforms',{'amd64':self.index['platforms']['amd64']}),
                            ('platforms',{'amd64':self.index['digest'],'arm64':self.index['platforms']['arm64']}),
                            ('digest','sha512:'+'a'*64),('digest',None)]:
            changed=copy.deepcopy(self.index);changed[field]=value;path.write_bytes(gate.canonical(changed));self.freeze()
            with self.subTest(field=field,value=value),patch.object(release_verify.subprocess,'run') as external,self.assertRaises(gate.Refused):self.call(self.index['digest'])
            external.assert_not_called()

    def test_every_authenticated_platform_and_index_has_exact_tag_and_source(self):
        for digest in [self.index['digest'],*self.index['platforms'].values()]:
            calls=[]
            def crypto(command,**kwargs):
                calls.append(command);return self.crypto(command,**kwargs)
            with self.subTest(digest=digest),patch('tag_signature.verify'),patch.object(release_verify.subprocess,'run',side_effect=crypto):self.call(digest)
            self.assertEqual(calls[-1][3],'oci://ghcr.io/marcinwadon/hennery@'+digest)
            self.assertIn('--bundle-from-oci',calls[-1]);self.assertNotIn('--signer-workflow',calls[-1])
            self.assertEqual(calls[-1][calls[-1].index('--cert-identity')+1],
                'https://github.com/'+gate.REPOSITORY+'/'+contract.WORKFLOW+'@refs/tags/v0.1.0')

    def test_source_metadata_cannot_change_between_root_and_image_authentication(self):
        changed=copy.deepcopy(self.inventory);changed['source_sha']='b'*40
        with patch.object(release_verify,'verify',return_value=changed),patch.object(release_verify,'verify_image_subject'),self.assertRaisesRegex(gate.Refused,'changed before'):
            self.call(self.index['digest'])

    def test_bytes_cannot_change_after_last_image_verification(self):
        calls=[]
        def changed(command,**kwargs):
            calls.append(command);result=self.crypto(command,**kwargs)
            if str(command[3]).startswith('oci://'):
                path=self.candidate/'ghcr-index.json';path.write_bytes(path.read_bytes()+b' ')
            return result
        with patch('tag_signature.verify'),patch.object(release_verify.subprocess,'run',side_effect=changed),self.assertRaises(gate.Refused):self.call(self.index['digest'])

    def test_local_manifest_must_match_authenticated_digest_and_have_bundle(self):
        helper=load_helper(self);helper.inspect_release(self.candidate,self.work,self.inventory)
        manifest=self.work/'layout/blobs/sha256'/self.index['digest'][7:]
        for bundle,data in [(None,manifest.read_bytes()),(self.root/'bundle',b'{}')]:
            manifest.write_bytes(data)
            with self.subTest(bundle=bundle),patch('tag_signature.verify'),patch.object(release_verify.subprocess,'run',side_effect=self.crypto),self.assertRaisesRegex(gate.Refused,'local manifest'):
                self.call(self.index['digest'],bundle=bundle,manifest=manifest)


if __name__ == '__main__': unittest.main()
