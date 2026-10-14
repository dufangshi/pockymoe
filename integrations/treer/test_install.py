"""Offline release fixtures; never installs into the real home or calls Treer."""
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import io
import json
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
import zipfile

import configure
import install


class PackagingTests(unittest.TestCase):
    def assets(self, corrupt=False, unsafe=False):
        binary = b"#!/bin/sh\nprintf '9.8.7\\n'\n"
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("../escape" if unsafe else "index.html", "fixture")
        web = buffer.getvalue()
        assets = {install.asset_name(): binary, "remote-codex-web.zip": web}
        sums = "\n".join(f"{hashlib.sha256(value).hexdigest()}  {key}" for key, value in assets.items())
        assets["SHA256SUMS"] = sums.encode()
        if corrupt:
            assets[install.asset_name()] = b"corrupt"
        return assets

    def fake_download(self, assets, seen):
        def download(url, destination, limit=install.LIMIT):
            self.assertIn("/releases/download/v9.8.7/", url)
            self.assertNotIn("latest", url)
            seen.append(url)
            destination.write_bytes(assets[url.rsplit("/", 1)[1]])
        return download

    def test_pinned_install_uses_native_layout_and_stable_links(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            seen = []
            with patch.object(install, "download", self.fake_download(self.assets(), seen)):
                cli = install.install("v9.8.7", home)
            self.assertEqual(len(seen), 3)
            native = home / ".local/share/remote-codex/native"
            self.assertEqual(cli.readlink(), native / "current/remote-codex")
            self.assertEqual((native / "current").readlink(), native / "releases/9.8.7")
            saved = json.loads((native / "current.json").read_text())
            self.assertEqual(saved["version"], "9.8.7")
            self.assertTrue((Path(saved["webDist"]) / "index.html").is_file())
            with self.assertRaisesRegex(ValueError, "already active"):
                install.install("9.8.8", home)
            self.assertFalse((native / ".treer-install.lock").exists())

    def test_corruption_and_traversal_never_activate(self):
        for corrupt, unsafe in [(True, False), (False, True)]:
            with self.subTest(corrupt=corrupt, unsafe=unsafe), tempfile.TemporaryDirectory() as temporary:
                home = Path(temporary)
                with patch.object(install, "download", self.fake_download(self.assets(corrupt, unsafe), [])):
                    with self.assertRaises(ValueError):
                        install.install("9.8.7", home)
                native = home / ".local/share/remote-codex/native"
                self.assertFalse((native / "current.json").exists())
                self.assertFalse((native / "releases/9.8.7").exists())
                self.assertFalse((home / ".local/bin/pockymoe").exists())

    def test_moving_versions_and_ambiguous_checksums_fail(self):
        for invalid in ["latest", "v1", "1.2.3/../../bad", "1.2.3-rc1"]:
            with self.assertRaises(ValueError):
                install.version(invalid)
        with self.assertRaises(ValueError):
            install.checksums(f"{'a' * 64} x\n{'b' * 64} x\n")

    def test_profiles_use_real_treer_flags_and_grok_acp(self):
        definitions = json.loads(Path(__file__).with_name("profiles.json").read_text())
        calls = configure.commands(definitions, Path("/stable/pockymoe"), Path("/data"), 4321, ".", "treer", True)
        self.assertEqual(calls[0][:5], ["treer", "app", "create", "--name", "pockymoe"])
        self.assertIn("POCKYMOE_HOSTED_BY=treer", calls[0])
        self.assertIn("POCKYMOE_SUPERVISOR_RESTART_STRATEGY=host-managed", calls[0])
        self.assertIn("/stable/pockymoe", calls[0])
        self.assertEqual(len(calls), 4)
        self.assertIn("acp", calls[3])
        self.assertIn("grok", calls[3])
        self.assertTrue(all("/data/treer-attach.token" in call for call in calls[1:]))

    def test_test_only_loopback_mirror_still_verifies_checksums(self):
        class QuietHandler(SimpleHTTPRequestHandler):
            def log_message(self, *_args):
                pass

        for corrupt in (False, True):
            with self.subTest(corrupt=corrupt), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                release = root / "mirror/v9.8.7"
                release.mkdir(parents=True)
                for name, content in self.assets(corrupt=corrupt).items():
                    (release / name).write_bytes(content)
                server = ThreadingHTTPServer(("127.0.0.1", 0),
                                            lambda *args, **kwargs: QuietHandler(*args, directory=root / "mirror", **kwargs))
                worker = threading.Thread(target=server.serve_forever, daemon=True)
                worker.start()
                try:
                    with patch.dict(install.os.environ, {"POCKYMOE_RELEASE_BASE_URL": f"http://127.0.0.1:{server.server_port}"}):
                        if corrupt:
                            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                                install.install("9.8.7", root / "home")
                            self.assertFalse((root / "home/.local/bin/pockymoe").exists())
                        else:
                            self.assertTrue(install.install("9.8.7", root / "home").is_file())
                finally:
                    server.shutdown()
                    worker.join()
                    server.server_close()

    def test_test_only_mirror_rejects_remote_http_and_credentials(self):
        for base in ("http://example.com", "https://user:secret@example.com", "file:///tmp",
                     "https://example.com?token=secret", "https://example.com#fragment"):
            with self.subTest(base=base), patch.dict(install.os.environ, {"POCKYMOE_RELEASE_BASE_URL": base}):
                with self.assertRaises(ValueError):
                    install.release_base("9.8.7")


if __name__ == "__main__":
    unittest.main()
