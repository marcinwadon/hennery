from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import contextlib
import io
import tarfile

import gate
import supply
import contract


class Supply(unittest.TestCase):
    def test_materials_manifest_location_is_independent_of_merge_order(self):
        manifest = b'{"schema":1,"node":{"version":"24.21.0"}}\n'
        locations = ['adapters/manifest.json', 'crates/hennery-host/manifest.json']
        for present in [locations[:1], locations[1:], locations]:
            with self.subTest(present=present), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                for location in present:
                    path = root / location
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(manifest)
                self.materials(root)
                runtime = gate.strict_json((root / 'out/runtime-inventory.json').read_bytes())
                identity = gate.strict_json((root / 'out/build-identity.json').read_bytes())
                self.assertEqual(runtime['manifest'], gate.strict_json(manifest))
                self.assertEqual(runtime['adapter_manifest_sha256'], gate.digest(manifest))
                self.assertEqual(identity['adapter_manifest_sha256'], gate.digest(manifest))

    def test_materials_refuses_disagreeing_adapter_manifest_locations(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for location, raw in [('adapters/manifest.json', b'{"schema":1}\n'),
                                  ('crates/hennery-host/manifest.json', b'{"schema":2}\n')]:
                path = root / location
                path.parent.mkdir(parents=True)
                path.write_bytes(raw)
            with self.assertRaisesRegex(ValueError, 'adapter manifest locations disagree'):
                self.materials(root)

    def test_materials_refuses_missing_adapter_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(ValueError, 'adapter manifest missing'):
                self.materials(Path(tmp))

    @staticmethod
    def materials(root):
        (root / 'metadata.json').write_text('{"packages":[],"workspace_members":[]}')
        (root / 'frontend.json').write_text('{}')
        (root / 'Cargo.lock').write_text('cargo lock fixture')
        (root / 'web/dist').mkdir(parents=True)
        (root / 'web/dist/index.html').write_text('UI fixture')
        (root / 'web/pnpm-lock.yaml').write_text('web lock fixture')
        argv = ['supply.py', 'materials', '--output', 'out', '--source-sha', 'a' * 40,
                '--target', 'aarch64-apple-darwin', '--cargo-metadata', 'metadata.json',
                '--frontend-licences', 'frontend.json']
        with contextlib.chdir(root), patch('sys.argv', argv):
            supply.main()

    def test_components_include_frontend_and_bundled_sources(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            rust = root / 'rust'
            rust.mkdir()
            (rust / 'LICENSE').write_text('Rust licence')
            (rust / 'Cargo.toml').write_text('[package]\nname="dependency"')
            openssl = rust / 'openssl'
            openssl.mkdir()
            (openssl / 'LICENSE.txt').write_text('OpenSSL Apache-2.0')
            web = root / 'frontend'
            web.mkdir()
            (web / 'LICENSE').write_text('Frontend licence')
            (web / 'package.json').write_text('{"name":"frontend","version":"1.2.3","license":"MIT"}')
            cargo = {'packages': [{'name': 'dependency', 'version': '1.0.0', 'license': 'MIT', 'source': 'registry+https://crates.io',
                                   'manifest_path': str(rust / 'Cargo.toml')}], 'workspace_members': []}
            frontend = {'MIT': [{'name': 'frontend', 'versions': ['1.2.3'], 'paths': [str(web)]}]}
            components, notices, findings = supply.components(cargo, frontend)
            self.assertEqual({c['name'] for c in components}, {'dependency', 'frontend'})
            self.assertIn('Frontend licence', notices)
            self.assertIn('OpenSSL Apache-2.0', notices)
            self.assertEqual(findings, [])
            sbom = supply.spdx(components, 'a' * 40, 'test-target')
            self.assertEqual(sbom['spdxVersion'], 'SPDX-2.3')
            self.assertEqual(len(sbom['packages']), 2)
            (web / 'LICENSE').unlink()
            self.assertTrue(supply.components(cargo, frontend)[2])

    def test_missing_each_channel_payload_refuses(self):
        for channels, tag in [(['github'], 'v0.1.0-alpha.1'), (['crates', 'github'], 'v0.1.0'), (['ghcr', 'github'], 'v0.1.0')]:
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                targets = ['aarch64-apple-darwin']
                for name in supply.required_payloads(channels, targets, tag):
                    (root / name).write_bytes(b'fixture')
                if 'crates' in channels:
                    self.write_cli_crate(root, tag)
                supply.require_contract(root, channels, targets, tag)
                for path in root.iterdir():
                    original = path.read_bytes()
                    path.unlink()
                    with self.subTest(channel=channels, missing=path.name), self.assertRaises(gate.Refused):
                        supply.require_contract(root, channels, targets, tag)
                    path.write_bytes(original)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in supply.required_payloads(['crates'], targets, 'v0.1.0-alpha.1'):
                (root / name).write_bytes(b'fixture')
            with self.assertRaises(gate.Refused):
                supply.require_contract(root, ['crates'], targets, 'v0.1.0-alpha.1')

    @staticmethod
    def write_cli_crate(root, tag, identity_ref=None):
        base='hennery-'+tag.removeprefix('v')
        identity_ref=identity_ref or 'refs/tags/'+tag
        text='[package]\nname="hennery"\nversion="'+tag.removeprefix('v')+'"\n[package.metadata.binstall]\ndisabled-strategies=["crate-meta-data","quick-install","compile"]\n[package.metadata.hennery-release]\nverification="external-signed-tag-sigstore"\nmaintainer-fingerprint="F26C7F594DFE84D131703AFC23E5A318AE8D2861"\nsigner-identity="https://github.com/marcinwadon/hennery/.github/workflows/release.yml@'+identity_ref+'"\n'
        with tarfile.open(root/(base+'.crate'),'w:gz') as archive:
            data=text.encode();entry=tarfile.TarInfo(base+'/Cargo.toml');entry.size=len(data);archive.addfile(entry,io.BytesIO(data))

    def test_crates_contract_rejects_branch_identity_in_frozen_metadata(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);channels=['crates','github'];targets=['aarch64-apple-darwin'];tag='v0.1.0'
            for name in supply.required_payloads(channels,targets,tag):(root/name).write_bytes(b'fixture')
            self.write_cli_crate(root,tag,'refs/heads/main')
            with self.assertRaisesRegex(gate.Refused,'exact release-tag identity'):
                supply.require_contract(root,channels,targets,tag)

    def test_freeze_checks_licences_and_freezes_complete_final_bytes_once(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            targets = ['aarch64-apple-darwin']
            for name in supply.required_payloads(['github'], targets, 'v0.1.0-alpha.1'):
                (root / name).write_bytes(b'fixture')
            base = 'hennery-' + targets[0]
            archive = root / (base + '.tar.xz')
            with tarfile.open(archive, 'w:xz') as tar:
                directory = tarfile.TarInfo(base + '/')
                directory.type = tarfile.DIRTYPE
                tar.addfile(directory)
                for name in ['hennery', 'CHANGELOG.md', 'LICENSE', 'README.md', 'THIRD-PARTY-NOTICES.txt']:
                    entry = tarfile.TarInfo(base + '/' + name)
                    entry.size = 1
                    tar.addfile(entry, io.BytesIO(b'x'))
            (root / (archive.name + '.sha256')).write_text(gate.digest(archive.read_bytes()) + '  ' + archive.name + '\n')
            (root / 'dist-manifest.json').write_text('{}')
            findings = root / 'licence-findings.json'
            findings.write_bytes(gate.canonical({'findings': ['unresolved']}))
            metadata = {'schema': 3, 'repository': gate.REPOSITORY, 'source_sha': 'a' * 40,
                        'tag': 'v0.1.0-alpha.1', 'tag_object': 'b' * 40, 'channels': ['github'],
                        'targets': targets}
            with self.assertRaisesRegex(gate.Refused, 'unresolved licence'):
                supply.freeze(root, metadata)
            findings.write_bytes(gate.canonical({'findings': []}))
            supply.freeze(root, metadata)
            inventory = contract.load(root)
            supply.verify_checksums(root)
            supply.verify_binary_inventory(inventory, root)
            wrong = dict(inventory, binaries={})
            with self.assertRaises(gate.Refused):
                supply.verify_binary_inventory(wrong, root)
            self.assertIn('release-metadata.json', [a['name'] for a in inventory['artifacts']])
            self.assertNotIn('artifacts', gate.strict_json((root / contract.METADATA).read_bytes()))
            with patch.object(contract, 'freeze') as generation, self.assertRaisesRegex(gate.Refused, 'already frozen'):
                supply.freeze(root, metadata)
            generation.assert_not_called()

    def test_supplemental_licence_hash_cannot_drift(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            directory = root / 'licences'
            directory.mkdir()
            (directory / 'text.txt').write_bytes(b'verbatim licence')
            entry = {'file': 'text.txt', 'sha256': gate.digest(b'verbatim licence'), 'source': 'immutable-upstream-revision'}
            (directory / 'manifest.json').write_bytes(gate.canonical({'cargo:dependency@1.0': [entry]}))
            with patch.object(supply, '__file__', str(root / 'supply.py')):
                self.assertEqual(supply.supplemental('cargo', 'dependency', '1.0')[0][1], 'verbatim licence')
                (directory / 'text.txt').write_bytes(b'changed licence')
                with self.assertRaises(gate.Refused):
                    supply.supplemental('cargo', 'dependency', '1.0')

    def test_source_and_runtime_digests(self):
        self.assertNotEqual(supply.tree_digest({'index.html': b'a'}), supply.tree_digest({'index.html': b'b'}))
        self.assertNotEqual(supply.tree_digest({'a': b'x'}), supply.tree_digest({'b': b'x'}))

    def test_checksum_inventory_is_complete_and_acyclic(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'payload').write_bytes(b'bytes')
            supply.checksums(root)
            self.assertEqual((root / 'SHA256SUMS').read_text(), gate.digest(b'bytes') + '  payload\n')
            supply.checksums(root)  # stable, never hashes itself
            self.assertNotIn('SHA256SUMS', (root / 'SHA256SUMS').read_text())
            (root / 'payload').write_bytes(b'changed')
            with self.assertRaises(gate.Refused):
                supply.verify_checksums(root)



    def test_crate_metadata_is_bounded_regular_and_unambiguous(self):
        for kind in ('duplicate','symlink','oversize'):
            with self.subTest(kind=kind),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);channels=['crates','github'];targets=['aarch64-apple-darwin'];tag='v0.1.0';base='hennery-0.1.0'
                for name in supply.required_payloads(channels,targets,tag):(root/name).write_bytes(b'fixture')
                self.write_cli_crate(root,tag)
                with tarfile.open(root/(base+'.crate'),'r:gz') as archive:data=archive.extractfile(base+'/Cargo.toml').read()
                if kind=='oversize':data+=b'\n#'+b'x'*(1024*1024)
                with tarfile.open(root/(base+'.crate'),'w:gz') as archive:
                    entry=tarfile.TarInfo(base+'/Cargo.toml')
                    if kind=='symlink':
                        entry.type=tarfile.SYMTYPE;entry.linkname='Cargo.toml.orig';archive.addfile(entry)
                        entry=tarfile.TarInfo(base+'/Cargo.toml.orig')
                    entry.size=len(data);archive.addfile(entry,io.BytesIO(data))
                    if kind=='duplicate':archive.addfile(entry,io.BytesIO(data))
                with tarfile.open(root/(base+'.crate'),'r:gz') as archive:
                    self.assertEqual(archive.extractfile(base+'/Cargo.toml').read(),data)
                with self.assertRaisesRegex(gate.Refused,'bounded regular file'):
                    supply.require_contract(root,channels,targets,tag)


if __name__ == '__main__':
    unittest.main()
