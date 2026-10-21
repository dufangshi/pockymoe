#!/usr/bin/env python3
import contextlib
import hashlib
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location('native_release', Path(__file__).with_name('prepare-runtime-release.py'))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)

class RuntimeReleaseTest(unittest.TestCase):
    def fixture(self, root):
        artifacts, web = root / 'artifacts', root / 'web'
        for platform, (binary, _) in release.PLATFORMS.items():
            target = artifacts / platform / binary
            target.parent.mkdir(parents=True)
            target.write_bytes(platform.encode())
        web.mkdir()
        (web / 'index.html').write_text('<html>Native web</html>')
        (web / 'assets').mkdir()
        (web / 'assets/app.js').write_text('console.log("web")')
        return artifacts, web

    def test_complete_deterministic_assets_and_checksums(self):
        with tempfile.TemporaryDirectory() as tmp, contextlib.redirect_stdout(io.StringIO()):
            root = Path(tmp)
            artifacts, web = self.fixture(root)
            first, second = root / 'first', root / 'second'
            release.prepare(artifacts, web, first, '0.12.75')
            release.prepare(artifacts, web, second, '0.12.75')
            self.assertEqual(len(list(first.iterdir())), 9)
            self.assertEqual((first / 'setup.sh').read_bytes(), Path(release.__file__).with_name('setup.sh').read_bytes())
            for asset in first.iterdir():
                self.assertEqual(asset.read_bytes(), (second / asset.name).read_bytes())
            for entry in (first / 'SHA256SUMS').read_text().splitlines():
                digest, name = entry.split()
                self.assertEqual(digest, hashlib.sha256((first / name).read_bytes()).hexdigest())
            with zipfile.ZipFile(first / 'remote-codex-web.zip') as archive:
                self.assertEqual(sorted(archive.namelist()), ['assets/app.js', 'index.html'])

    def test_partial_platform_release_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            artifacts, web = self.fixture(root)
            (artifacts / 'linux-arm64-gnu/pockymoe').unlink()
            with self.assertRaisesRegex(ValueError, 'Missing native release artifact'):
                release.prepare(artifacts, web, root / 'output', '0.12.75')
            self.assertFalse((root / 'output').exists())

if __name__ == '__main__':
    unittest.main()
