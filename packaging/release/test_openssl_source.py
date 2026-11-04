import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import gate
import openssl_source
import supply


class SupportedOpenSSL(unittest.TestCase):
    def test_corrupt_archive_is_refused_before_extraction_or_build(self):
        with tempfile.TemporaryDirectory() as tmp:
            archive = Path(tmp) / 'source.tar.gz'
            archive.write_bytes(b'not the approved source')
            with self.assertRaisesRegex(ValueError, 'SHA-256'):
                openssl_source.verify_archive(archive, 'a' * 64)
            openssl_source.verify_archive(archive, hashlib.sha256(archive.read_bytes()).hexdigest())

    def test_expired_support_pin_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = Path(tmp) / 'openssl.json'
            manifest.write_text(json.dumps(openssl_source.source() | {'supported_until': '2000-01-01'}))
            with patch.object(openssl_source, 'SOURCE', manifest):
                with self.assertRaisesRegex(ValueError, 'support has expired'):
                    openssl_source.source()

    def test_wrong_or_missing_library_in_archive_binary_is_refused(self):
        with patch.object(openssl_source.check_source, 'version_output') as version:
            for content in ('no native library', 'crypto: OpenSSL 3.6.3 9 Jun 2026',
                            'crypto: OpenSSL 3.5.8 25 Aug 2026',
                            'crypto: OpenSSL 3.5.9 fixture\ncrypto: OpenSSL 3.5.9 fixture'):
                version.return_value = content
                with self.subTest(content=content), self.assertRaisesRegex(ValueError, 'pinned OpenSSL'):
                    openssl_source.check_binary('unused-fixture-path')
            version.return_value = 'crypto: OpenSSL ' + openssl_source.source()['version'] + ' fixture'
            openssl_source.check_binary('unused-fixture-path')

    def test_native_notices_and_sbom_bind_the_reviewed_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            prefix = Path(tmp)
            pin = openssl_source.source()
            (prefix / 'hennery-source.json').write_text(json.dumps(pin))
            licence = prefix / 'LICENSE.txt'
            licence.write_text('OpenSSL Apache-2.0 licence fixture')
            package, notice = supply.static_openssl(prefix)
            self.assertEqual(package['versionInfo'], pin['version'])
            self.assertEqual(package['downloadLocation'], pin['url'])
            self.assertEqual(package['checksums'], [{'algorithm': 'SHA256', 'checksumValue': pin['sha256']}])
            self.assertIn(licence.read_text(), notice)
            self.assertIn(pin['sha256'], notice)
            licence.unlink()
            with self.assertRaisesRegex(gate.Refused, 'licence missing'):
                supply.static_openssl(prefix)
            (prefix / 'hennery-source.json').write_text(json.dumps(pin | {'version': '3.6.3'}))
            with self.assertRaisesRegex(gate.Refused, 'differs from reviewed pin'):
                supply.static_openssl(prefix)
