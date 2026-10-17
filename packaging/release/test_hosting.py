"""Real pinned gh release client against a loopback-only TLS service.

Exercises CLI-produced JSON, hosting lifecycle and downloaded bytes.
The service is a stand-in: this cannot qualify GitHub immutability or approval.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import ssl
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, unquote, urlsplit

import contract
import gate
import hosting
import release_run
import verifier_process

FIXTURES = Path(__file__).with_name('fixtures')
ALPHA = FIXTURES / 'alpha3'


class Hosting(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix='hosting-cli-')
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.directory = Path(cls.temporary.name)
        configuration = cls.directory / 'openssl.cnf'
        configuration.write_text('[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n'
                                 '[dn]\nCN=localhost\n[ext]\nsubjectAltName=DNS:localhost\n'
                                 'basicConstraints=critical,CA:TRUE\n')
        cls.certificate = cls.directory / 'cert.pem'
        key = cls.directory / 'key.pem'
        subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
                        '-keyout', str(key), '-out', str(cls.certificate), '-config', str(configuration)],
                       check=True, capture_output=True)
        cls.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cls.certificate, key)
        cls.server.socket = context.wrap_socket(cls.server.socket, server_side=True)
        cls.server.daemon_threads = True
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.addClassCleanup(cls.server.server_close)
        cls.addClassCleanup(cls.thread.join)
        cls.addClassCleanup(cls.server.shutdown)
        cls.origin = 'https://localhost:' + str(cls.server.server_port)
        cls.repository = 'localhost:' + str(cls.server.server_port) + '/' + gate.REPOSITORY
        with verifier_process.environment() as env:
            version = subprocess.check_output(['gh', '--version'], env=env, text=True)
        if not version.startswith('gh version 2.101.0 '):
            raise AssertionError('requalify gh hosting boundary: ' + version)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root, self.signatures = Path(self.tmp.name) / 'payload', Path(self.tmp.name) / 'signatures'
        self.root.mkdir(); self.signatures.mkdir()
        for name in (contract.CHECKSUMS, contract.METADATA):
            shutil.copyfile(ALPHA / name, self.root / name)
        shutil.copyfile(ALPHA / contract.BUNDLE, self.signatures / contract.BUNDLE)
        self.metadata = gate.strict_json((self.root / contract.METADATA).read_bytes())
        self.remote, self.assets, self.writes, self.fault = None, {}, [], None
        self.server.case = self
        self.real_run = subprocess.run

        def local_transport(command, **kwargs):
            if command[:2] == ['gh', 'release']:
                # Only replace transport origin. The production commands, flags,
                # real CLI parser and response serializer remain intact.
                command = list(command)
                command[command.index('--repo') + 1] = self.repository
                env = kwargs['env']
                self.assertEqual(env.pop('GH_TOKEN'), 'offline-hosting-fixture')
                self.assertFalse(any(k in env for k in ('GITHUB_TOKEN', 'GHCR_PUBLISH_TOKEN',
                                                       'CARGO_REGISTRY_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN')))
                env.update(GH_ENTERPRISE_TOKEN='offline-hosting-fixture', SSL_CERT_FILE=str(self.certificate),
                           HTTPS_PROXY='http://127.0.0.1:9', HTTP_PROXY='http://127.0.0.1:9',
                           ALL_PROXY='http://127.0.0.1:9', NO_PROXY='localhost,127.0.0.1')
            return self.real_run(command, **kwargs)

        self.patch = patch.object(hosting.subprocess, 'run', side_effect=local_transport)
        self.patch.start(); self.addCleanup(self.patch.stop)
        credentials = patch.dict(os.environ, {'GH_TOKEN': 'offline-hosting-fixture',
                                              'CARGO_REGISTRY_TOKEN': 'excluded-canary',
                                              'ACTIONS_ID_TOKEN_REQUEST_TOKEN': 'excluded-canary'})
        credentials.start(); self.addCleanup(credentials.stop)

    def authorize(self):
        # Real public producer output is authenticated under the production
        # consumer flags, with offline trust root and blocked network.
        with verifier_process.environment() as env:
            env.update(HTTPS_PROXY='http://127.0.0.1:9', HTTP_PROXY='http://127.0.0.1:9', NO_PROXY='')
            for subject in (contract.CHECKSUMS, contract.METADATA):
                result = self.real_run(contract.attestation_command(self.root / subject, self.metadata['tag'],
                    self.metadata['source_sha'], self.signatures / contract.BUNDLE,
                    FIXTURES / 'attestation/trusted_root.json'), env=env, capture_output=True, text=True)
                gate.require(result.returncode == 0, 'real fixture authorization failed: ' + result.stderr)
        return self.metadata

    def publish(self):
        return hosting.release(self.root, self.signatures, self.authorize)

    def record(self, draft=True, immutable=False):
        return {'id': 7, 'node_id': 'RE_fixture', 'tag_name': self.metadata['tag'], 'name': self.metadata['tag'],
                'draft': draft, 'immutable': immutable, 'prerelease': True,
                'target_commitish': self.metadata['source_sha'], 'body': 'fixture',
                'url': self.origin + '/api/v3/repos/' + gate.REPOSITORY + '/releases/7',
                'html_url': self.origin + '/release/7', 'upload_url': self.origin + '/upload{?name,label}'}

    def test_real_cli_publishes_exact_genuine_bytes_and_retries_without_writes(self):
        result = self.publish()
        self.assertEqual(result['state'], 'immutable')
        self.assertTrue(self.remote['immutable'])
        expected = {p.name: p.read_bytes() for folder in (self.root, self.signatures) for p in folder.iterdir()}
        self.assertEqual(self.assets, expected)
        self.assertTrue(self.writes[0][1]['draft'])
        self.assertEqual(self.writes[0][1]['make_latest'], 'false')
        self.assertEqual(self.writes[-1][1]['make_latest'], 'false')
        self.writes.clear()
        self.publish()
        self.assertEqual(self.writes, [])
        release_run.require_public(self.root, self.signatures, self.metadata)

    def test_partial_draft_uploads_only_missing_identical_bytes(self):
        self.remote = self.record()
        self.assets[contract.CHECKSUMS] = (self.root / contract.CHECKSUMS).read_bytes()
        self.publish()
        uploads = [body for method, body in self.writes if method == 'upload']
        self.assertEqual(set(uploads), {contract.METADATA, contract.BUNDLE})

    def test_remote_conflicts_and_incomplete_immutable_release_never_write(self):
        for fault in ('wrong-bytes', 'extra', 'duplicate', 'missing-immutable', 'mutable', 'wrong-tag', 'wrong-prerelease'):
            with self.subTest(fault=fault):
                self.remote = self.record(draft=fault not in ('missing-immutable', 'mutable'),
                                          immutable=fault == 'missing-immutable')
                self.assets = {contract.CHECKSUMS: (self.root / contract.CHECKSUMS).read_bytes()}
                self.writes.clear(); self.fault = fault
                if fault == 'wrong-bytes': self.assets[contract.CHECKSUMS] = b'conflict'
                if fault == 'extra': self.assets['unapproved'] = b'extra'
                if fault == 'wrong-tag': self.remote['tag_name'] = 'v9.0.0'
                if fault == 'wrong-prerelease': self.remote['prerelease'] = False
                with self.assertRaises(gate.Refused): self.publish()
                self.assertEqual(self.writes, [])

    def test_cli_auth_or_transport_failure_is_not_absence(self):
        for fault in ('unauthorized', 'unavailable'):
            with self.subTest(fault=fault):
                self.fault = fault
                with self.assertRaises(gate.Refused): self.publish()
                self.assertEqual(self.writes, [])

    def test_corrupt_upload_stops_before_publication(self):
        self.fault = 'corrupt-upload'
        with self.assertRaises(gate.Refused): self.publish()
        self.assertTrue(self.remote['draft'])
        self.assertFalse(any(method == 'publish' for method, _ in self.writes))

    def test_lost_publish_response_is_reconciled_without_reupload(self):
        self.fault = 'lost-response'
        with self.assertRaises(gate.Refused): self.publish()
        self.assertTrue(self.remote['immutable'])
        self.fault = None; self.writes.clear()
        self.publish()
        self.assertEqual(self.writes, [])

    def test_disabled_immutability_is_detected_after_publication(self):
        self.fault = 'no-immutability'
        with self.assertRaises(gate.Refused): self.publish()
        self.assertFalse(self.remote['immutable'])
        with self.assertRaises(gate.Refused):
            release_run.require_public(self.root, self.signatures, self.metadata)

    def test_changed_genuine_input_prevents_any_write(self):
        (self.root / contract.METADATA).write_bytes((self.root / contract.METADATA).read_bytes() + b'changed')
        with self.assertRaises(gate.Refused): self.publish()
        self.assertEqual(self.writes, [])

    def test_missing_remote_tag_cannot_be_created_by_release_hosting(self):
        self.fault = 'missing-tag'
        with self.assertRaises(gate.Refused): self.publish()
        self.assertEqual(self.writes, [])

    def test_byte_change_after_verification_prevents_first_write(self):
        calls = 0

        def authorize():
            nonlocal calls
            metadata = self.authorize()
            calls += 1
            if calls == 2:
                with (self.signatures / contract.BUNDLE).open('ab') as bundle:
                    bundle.write(b'changed after verification')
            return metadata

        with self.assertRaises(gate.Refused):
            hosting.release(self.root, self.signatures, authorize)
        self.assertEqual(self.writes, [])

    def test_missing_or_unsafe_local_sidecar_never_writes(self):
        for name in ('extra.json', 'unsafe#label'):
            with self.subTest(name=name):
                path = self.signatures / name; path.write_bytes(b'extra')
                with self.assertRaises(gate.Refused): self.publish()
                path.unlink()
                self.assertEqual(self.writes, [])

    def test_channel_requires_complete_immutable_public_bytes_including_bundles(self):
        for draft, immutable in ((True, False), (False, True)):
            self.remote = self.record(draft=draft, immutable=immutable)
            self.assets = {contract.CHECKSUMS: (self.root / contract.CHECKSUMS).read_bytes()}
            with self.subTest(draft=draft), self.assertRaises(gate.Refused):
                release_run.require_public(self.root, self.signatures, self.metadata)
        self.assets = {p.name: p.read_bytes() for folder in (self.root, self.signatures) for p in folder.iterdir()}
        self.assets[contract.BUNDLE] += b'changed'
        with self.assertRaises(gate.Refused):
            release_run.require_public(self.root, self.signatures, self.metadata)
        self.assertEqual(self.writes, [])


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, status, value):
        body = value if isinstance(value, bytes) else json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json' if not isinstance(value, bytes) else 'application/octet-stream')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers(); self.wfile.write(body)

    def do_GET(self):
        case = self.server.case
        if case.fault in ('unauthorized', 'unavailable'):
            return self.reply(401 if case.fault == 'unauthorized' else 503, {'message': case.fault})
        path = urlsplit(self.path).path
        if path.startswith('/download/'):
            return self.reply(200, case.assets[unquote(path.removeprefix('/download/'))])
        if path.endswith('/releases'):
            return self.reply(200, [] if case.remote is None else [self.record()])
        if '/git/ref/tags/' in path:
            if case.fault == 'missing-tag': return self.reply(404, {'message': 'Not Found'})
            return self.reply(200, {'ref': 'refs/tags/' + case.metadata['tag'], 'object': {'sha': case.metadata['tag_object']}})
        if '/releases/tags/' in path or path.endswith('/releases/7'):
            return self.reply(404, {'message': 'Not Found'}) if case.remote is None else self.reply(200, self.record())
        return self.reply(404, {'message': 'unknown fixture route: ' + path})

    def record(self):
        case = self.server.case
        assets = [{'id': number, 'name': name, 'size': len(body), 'state': 'uploaded',
                   'url': case.origin + '/download/' + name, 'browser_download_url': case.origin + '/download/' + name}
                  for number, (name, body) in enumerate(case.assets.items(), 1)]
        if case.fault == 'duplicate': assets += assets[:1]
        return case.remote | {'assets': assets}

    def do_POST(self):
        case = self.server.case
        body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
        path = urlsplit(self.path)
        if path.path == '/upload':
            name = parse_qs(path.query)['name'][0]
            case.writes.append(('upload', name))
            if name in case.assets: return self.reply(422, {'message': 'already exists'})
            case.assets[name] = body + (b'changed' if case.fault == 'corrupt-upload' else b'')
            return self.reply(201, {'id': len(case.assets), 'name': name, 'state': 'uploaded'})
        if path.path.endswith('/releases'):
            value = json.loads(body); case.writes.append(('create', value))
            case.remote = case.record() | value
            return self.reply(201, self.record())
        if path.path.endswith('/graphql'):
            return self.reply(200, {'data': {'repository': {'ref': None if case.fault == 'missing-tag' else {'id': 'tag-fixture'}}}})
        return self.reply(404, {'message': 'unknown fixture route: ' + path.path})

    def do_PATCH(self):
        case = self.server.case
        value = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        case.writes.append(('publish', value))
        case.remote.update(value, immutable=case.fault != 'no-immutability')
        if case.fault == 'lost-response':
            return self.reply(503, {'message': 'response lost after publication'})
        return self.reply(200, self.record())
