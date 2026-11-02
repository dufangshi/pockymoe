#!/usr/bin/env python3
import hashlib
import importlib.util
import io
from pathlib import Path
import unittest
import zipfile

spec = importlib.util.spec_from_file_location('guard', Path(__file__).with_name('check-production-address.py'))
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class AddressGuardTest(unittest.TestCase):
    forbidden = {hashlib.sha256(b'private.example.test').hexdigest(): len(b'private.example.test')}

    def test_binary_utf16_and_escaped_subdomains(self):
        for text in ['https://relay.private.example.test/path', 'PRIVATE.EXAMPLE.TEST', r'private\.example\.test', r'private\u002eexample.test', r'\u0050rivate.example.test', 'prefixprivate.example.testsuffix']:
            for encoding in ['utf-8', 'utf-16-le', 'utf-16-be']:
                with self.subTest(encoding=encoding, text=text):
                    self.assertTrue(guard.contains_address(text.encode(encoding), self.forbidden))
        self.assertFalse(guard.contains_address(b'https://relay.example.com', self.forbidden))

    def test_compressed_zip_and_member_names_are_scanned_without_echoing_contents(self):
        for name, data in [('app.js', b'private.example.test'), ('private.example.test.txt', b'neutral')]:
            archive = io.BytesIO()
            with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED) as target:
                target.writestr(name, data)
            with self.assertRaises(ValueError) as caught:
                guard.scan_blob(archive.getvalue(), 'test archive', self.forbidden)
            self.assertNotIn('private.example.test', str(caught.exception))

    def test_manifest_exception_is_frozen(self):
        path = Path(__file__).resolve().parent.parent / guard.FROZEN_MANIFEST
        self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), guard.FROZEN_MANIFEST_HASH)
        self.assertTrue(guard.contains_address(path.read_bytes()))


if __name__ == '__main__':
    unittest.main()
