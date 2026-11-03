"""The privileged publisher treats generated artifacts as untrusted data."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('copy_pin_update', Path(__file__).parents[1] / 'copy_pin_update.py')
copy = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(copy)


class CopyPinUpdate(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.artifact, self.checkout = self.root / 'artifact', self.root / 'checkout'
        for relative in copy.FILES:
            for root, data in [(self.artifact, b'new pin data'), (self.checkout, b'old pin data')]:
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)

    def test_extra_git_metadata_and_executables_never_enter_checkout(self):
        for relative in ['.git/config', '.git/hooks/pre-commit', 'packaging/copy_pin_update.py']:
            path = self.artifact / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('hostile artifact content')
        copy.copy_update(self.artifact, self.checkout)
        for relative in copy.FILES:
            self.assertEqual((self.checkout / relative).read_bytes(), b'new pin data')
        self.assertFalse((self.checkout / '.git').exists())
        self.assertFalse((self.checkout / 'packaging').exists())

    def assert_checkout_unchanged(self):
        for relative in copy.FILES:
            self.assertEqual((self.checkout / relative).read_bytes(), b'old pin data')

    def test_missing_final_file_refuses_before_any_write(self):
        (self.artifact / copy.FILES[-1]).unlink()
        with self.assertRaises(FileNotFoundError):
            copy.copy_update(self.artifact, self.checkout)
        self.assert_checkout_unchanged()

    def test_source_symlink_cannot_read_outside_artifact(self):
        path = self.artifact / copy.FILES[-1]
        path.unlink()
        path.symlink_to(self.checkout / copy.FILES[0])
        with self.assertRaises(ValueError):
            copy.copy_update(self.artifact, self.checkout)
        self.assert_checkout_unchanged()

    def test_destination_symlink_cannot_write_outside_checkout(self):
        outside = self.root / 'outside'
        outside.write_bytes(b'preserve')
        path = self.checkout / copy.FILES[-1]
        path.unlink()
        path.symlink_to(outside)
        with self.assertRaises(ValueError):
            copy.copy_update(self.artifact, self.checkout)
        self.assertEqual(outside.read_bytes(), b'preserve')
        self.assertEqual((self.checkout / copy.FILES[0]).read_bytes(), b'old pin data')

    def test_oversized_data_is_refused_before_any_write(self):
        with (self.artifact / copy.FILES[-1]).open('wb') as stream:
            stream.truncate(copy.MAX_FILE_BYTES + 1)
        with self.assertRaises(ValueError):
            copy.copy_update(self.artifact, self.checkout)
        self.assert_checkout_unchanged()
