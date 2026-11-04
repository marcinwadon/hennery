"""Real pnpm licence output survives transfer to an archive runner without deps."""
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import unittest

import assets
import gate
import openssl_source

ROOT = Path(__file__).resolve().parents[2]


@contextmanager
def local_registry(package):
    # Registry versions exercise pnpm's production report shape; file: deps
    # report the file: specifier as their version instead of the package version.
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode='w:gz') as archive:
        archive.add(package, arcname='package')
    tarball = output.getvalue()
    manifest = json.loads((package / 'package.json').read_text())

    class Registry(BaseHTTPRequestHandler):
        def log_message(self, *args): pass

        def do_GET(self):
            body = tarball if self.path.endswith('.tgz') else json.dumps({
                'name': manifest['name'], 'dist-tags': {'latest': manifest['version']},
                'versions': {manifest['version']: manifest | {'dist': {
                    'tarball': origin + '/frontend-fixture-1.2.3.tgz',
                    'shasum': hashlib.sha1(tarball).hexdigest()}}}}).encode()
            self.send_response(200)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers(); self.wfile.write(body)

    server = ThreadingHTTPServer(('127.0.0.1', 0), Registry)
    origin = 'http://127.0.0.1:' + str(server.server_port)
    thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
    try:
        yield origin
    finally:
        server.shutdown(); thread.join(); server.server_close()


class FrontendMaterials(unittest.TestCase):
    def test_archive_materials_keep_notices_and_combined_sbom_without_node_modules_or_source_archives(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            ui, archive = root / 'ui-job', root / 'archive-job'
            ui.mkdir(); archive.mkdir()
            dependency = ui / 'dependency'; dependency.mkdir()
            (dependency / 'package.json').write_text(json.dumps({
                'name': 'frontend-fixture', 'version': '1.2.3', 'license': 'MIT'}))
            (dependency / 'LICENSE').write_text('frontend fixture licence text\n')
            (dependency / 'index.js').write_text('export const fixture = true;\n')
            (ui / 'package.json').write_text(json.dumps({
                'name': 'ui-fixture', 'private': True, 'version': '1.0.0',
                'dependencies': {'frontend-fixture': '1.2.3'}}))
            home = root / 'home'; home.mkdir()
            env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / 'config'),
                       XDG_CACHE_HOME=str(home / 'cache'), npm_config_userconfig=str(home / 'npmrc'),
                       npm_config_cache=str(home / 'npm-cache'))
            script = ('export HTTP_PROXY=http://127.0.0.1:9 HTTPS_PROXY=http://127.0.0.1:9 NO_PROXY=127.0.0.1 CI=1; '
                      'pnpm --dir "$1" install --ignore-scripts --store-dir "$2" --registry "$3" >&2; '
                      'pnpm --dir "$1" licenses list --json')
            with local_registry(dependency) as registry:
                result = subprocess.run(['nix', 'develop', '.#web', '-c', 'bash', '-eu', '-c', script,
                                         '--', str(ui), str(root / 'store'), registry], cwd=ROOT, env=env,
                                        capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            licences = ui / 'frontend-licences.json'; licences.write_text(result.stdout)
            installed = Path(gate.strict_json(licences.read_bytes())['MIT'][0]['paths'][0])
            prepared = ui / 'frontend'
            result = subprocess.run([sys.executable, str(ROOT / 'packaging/release/supply.py'),
                'frontend', '--output', str(prepared), '--frontend-licences', str(licences)],
                cwd=ui, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr + '\n' + licences.read_text())
            shutil.copytree(prepared, archive / 'inputs/frontend')
            # A missing licence remains a finding across the same transfer.
            (installed / 'LICENSE').unlink()
            missing = ui / 'frontend-missing'
            result = subprocess.run([sys.executable, str(ROOT / 'packaging/release/supply.py'),
                'frontend', '--output', str(missing), '--frontend-licences', str(licences)],
                cwd=ui, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            shutil.copytree(missing, archive / 'missing-frontend')
            # The original absolute pnpm paths must be unusable after transfer.
            shutil.rmtree(ui)
            (archive / 'packaging').symlink_to(ROOT / 'packaging', target_is_directory=True)
            (archive / 'adapters').mkdir()
            (archive / 'adapters/manifest.json').write_text('{"schema":1}\n')
            (archive / 'cargo-metadata.json').write_text('{"packages":[],"workspace_members":[]}')
            (archive / 'Cargo.toml').write_text('[workspace.package]\nversion="0.1.0"\n')
            (archive / 'Cargo.lock').write_text('cargo lock fixture\n')
            (archive / 'web/dist').mkdir(parents=True)
            (archive / 'web/dist/index.html').write_text('prepared UI\n')
            (archive / 'web/pnpm-lock.yaml').write_text('web lock fixture\n')
            advisories = archive / 'inputs/advisories'; advisories.mkdir()
            for name in ('rust-advisories.json', 'rust-advisory-revision.txt', 'web-advisories.json'):
                (advisories / name).write_text('prepared advisory\n')
            self.assertFalse((archive / 'web/node_modules').exists())
            native = archive / 'openssl'; native.mkdir()
            (native / 'hennery-source.json').write_text(json.dumps(openssl_source.source()))
            (native / 'LICENSE.txt').write_text('OpenSSL Apache-2.0 fixture')
            result = subprocess.run(['sh', str(ROOT / 'packaging/release/native-materials.sh')],
                cwd=archive, env=env | {'RELEASE_SOURCE_SHA': 'a' * 40, 'TARGET': 'aarch64-apple-darwin', 'OPENSSL_DIR': str(native)},
                capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            materials = archive / 'release-materials'
            self.assertIn('frontend fixture licence text', (materials / 'THIRD-PARTY-NOTICES.txt').read_text())
            packages = gate.strict_json((materials / 'hennery-aarch64-apple-darwin.spdx.json').read_bytes())['packages']
            self.assertEqual([(p['name'], p['versionInfo']) for p in packages],
                             [('OpenSSL', openssl_source.source()['version']), ('frontend-fixture', '1.2.3')])
            self.assertEqual(gate.strict_json((materials / 'licence-findings.json').read_bytes())['findings'], [])
            notices = (materials / 'THIRD-PARTY-NOTICES.txt').read_text()
            self.assertIn('AGPL-3.0-only', notices)
            self.assertIn('Signed release tag: https://github.com/marcinwadon/hennery/tree/v0.1.0', notices)
            self.assertIn('https://github.com/marcinwadon/hennery/archive/refs/tags/v0.1.0.tar.gz', notices)
            self.assertIn('https://github.com/marcinwadon/hennery/archive/refs/tags/v0.1.0.zip', notices)
            self.assertFalse(any(p.name.endswith(('.tar.xz', '.tar.gz')) for p in materials.iterdir()))
            # The combined SBOM consumes the real pnpm producer output after
            # the original node_modules has been removed.
            public = root / 'public'; public.mkdir()
            sha = 'a' * 40
            for target in sorted(gate.TARGETS):
                shutil.copyfile(materials / 'hennery-aarch64-apple-darwin.spdx.json', public / ('hennery-' + target + '.spdx.json'))
                (public / ('hennery-' + target + '.tar.xz')).write_bytes(b'native archive input')
            (public / 'hennery-installer.sh').write_text('installer input')
            result = subprocess.run([sys.executable, str(ROOT / 'packaging/release/assets.py'), str(public), sha,
                                     '--targets', ','.join(sorted(gate.TARGETS))], cwd=archive, env=env,
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            files = assets.files(public, sorted(gate.TARGETS))
            self.assertEqual(len(files), 6)
            self.assertEqual(files['SHA256SUMS'].read_bytes(), (public / assets.PUBLIC_CHECKSUMS).read_bytes())
            self.assertEqual({p['name'] for p in gate.strict_json((public / assets.SBOM).read_bytes())['packages']}, {'OpenSSL', 'frontend-fixture'})
            shutil.copytree(archive / 'missing-frontend', archive / 'inputs/frontend', dirs_exist_ok=True)
            result = subprocess.run(['sh', str(ROOT / 'packaging/release/native-materials.sh')],
                cwd=archive, env=env | {'RELEASE_SOURCE_SHA': 'a' * 40, 'TARGET': 'aarch64-apple-darwin', 'OPENSSL_DIR': str(native)},
                capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(gate.strict_json((materials / 'licence-findings.json').read_bytes())['findings'],
                             ['npm:frontend-fixture@1.2.3: missing licence expression or text'])
