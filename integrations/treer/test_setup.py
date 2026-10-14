"""Offline POSIX setup tests; every command/state lives in a temporary fake home."""
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import unittest

import bundle_setup
import configure

SOURCE = Path(__file__).parent
FAKE_TREER = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys
state_path = Path(os.environ["FAKE_STATE"])
state = json.loads(state_path.read_text())
args = sys.argv[1:]
state["calls"].append(args)
def finish(value=None, code=0):
    state_path.write_text(json.dumps(state))
    if value is not None:
        print(json.dumps(value))
    if code:
        print("PRIVATE_SUBPROCESS_DIAGNOSTIC", file=sys.stderr)
    sys.exit(code)
def flag(name):
    return args[args.index(name) + 1]
if args == ["whoami"]:
    if state["identityFailures"]:
        state["identityFailures"] -= 1
        finish(code=1)
    finish({"machine": {"server_id": "test-machine", "root": os.environ["HOME"]},
            "privateFixture": "PRIVATE_SUBPROCESS_DIAGNOSTIC"})
if args == ["interface", "contract"]:
    finish(state["contract"])
if args == ["app", "list"]:
    finish({"apps": state["apps"]})
if args == ["agent", "admin", "profile", "list"]:
    finish({"profiles": state["profiles"]})
if args[:2] == ["app", "create"]:
    split = args.index("--")
    state["apps"].append({"app_id": "hub-id", "server_id": "test-machine", "name": flag("--name"),
                           "port": int(flag("--port")), "cwd": flag("--cwd"),
                           "command": args[split - 1], "args": args[split + 1:]})
    finish()
if args[:4] == ["agent", "admin", "profile", "create"]:
    if state.get("failProfile") == args[4]:
        state.pop("failProfile")
        finish(code=1)
    split = args.index("--")
    state["profiles"].append({"profile_id": "profile-" + str(len(state["profiles"])), "name": args[4],
                               "description": flag("--description"), "cwd": flag("--cwd"),
                               "command": args[split - 1], "args": args[split + 1:]})
    finish()
if args[:4] == ["agent", "admin", "profile", "update"]:
    profile = next(p for p in state["profiles"] if p["profile_id"] == args[4])
    profile.update(description=flag("--description"), cwd=flag("--cwd"), command=flag("--command"),
                   args=[a[len("--arg="):] for a in args if a.startswith("--arg=")])
    finish()
finish(code=1)
'''
FAKE_INSTALL_MAIN = '''if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    binary = Path.home() / ".local/bin/pockymoe"
    binary.parent.mkdir(parents=True, exist_ok=True)
    binary.write_text("#!/bin/sh\\nprintf '%s\\\\n' '" + args.version + "'\\n")
    binary.chmod(0o700)
    (Path.home() / "installer-version").write_text(args.version)
'''


class SetupTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="pockymoe setup ")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.home = self.root / "home"
        self.home.mkdir()
        self.bundle = self.root / "uploaded files"
        self.bundle.mkdir()
        for name in ("setup.sh", "install.py", "configure.py", "profiles.json"):
            shutil.copyfile(SOURCE / name, self.bundle / name)
        installer = self.bundle / "install.py"
        installer.write_text(installer.read_text().replace('if __name__ == "__main__":\n    sys.exit(main())\n', FAKE_INSTALL_MAIN))
        self.script = self.root / "treer-setup.sh"
        self.script.write_text(bundle_setup.generate(self.bundle))
        # The public script must work with no adjacent sources or downloaded scripts.
        shutil.rmtree(self.bundle)
        self.temp_dir = self.root / "private extraction"
        self.temp_dir.mkdir()
        self.bin_dir = self.root / "bin"
        self.bin_dir.mkdir()
        treer = self.bin_dir / "treer"
        treer.write_text(FAKE_TREER)
        treer.chmod(0o700)
        self.state_path = self.root / "state.json"
        self.save({"apps": [], "profiles": [], "calls": [], "identityFailures": 0,
                   "contract": {"protocol": "treer.agent-interface/v1", "capabilities": [],
                                "features": ["agent.interface-provenance.v1", "agent.prompt-delivery.v1"]}})
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(("TREER_", "POCKYMOE_", "FAKE_"))}
        self.env.update(HOME=str(self.home), PATH=str(self.bin_dir) + os.pathsep + os.environ["PATH"],
                        TREER_AGENT_ID="installer-fixture", FAKE_STATE=str(self.state_path),
                        PYTHONDONTWRITEBYTECODE="1", TMPDIR=str(self.temp_dir))

    def save(self, state):
        self.state_path.write_text(json.dumps(state))

    def state(self):
        return json.loads(self.state_path.read_text())

    def installed(self, version="0.13.2"):
        binary = self.home / ".local/bin/pockymoe"
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_text(f"#!/bin/sh\nprintf '%s\\n' '{version}'\n")
        binary.chmod(0o700)
        return binary

    def run_setup(self, *args, success=True):
        result = subprocess.run(["sh", str(self.script), *args], env=self.env,
                                cwd=self.root, capture_output=True, text=True, timeout=15)
        self.assertEqual(list(self.temp_dir.iterdir()), [], "Private extraction must be removed even on failure")
        self.assertNotIn("PRIVATE_SUBPROCESS_DIAGNOSTIC", result.stdout + result.stderr)
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def test_fresh_install_via_fake_installer_and_summary(self):
        data = self.root / "custom data with spaces"
        result = self.run_setup("--version", "0.13.2", "--hub-port", "18787", "--data-dir", str(data))
        self.assertEqual((self.home / "installer-version").read_text(), "0.13.2")
        self.assertEqual(len(self.state()["apps"]), 1)
        self.assertEqual(len(self.state()["profiles"]), 3)
        self.assertIn("App pockymoe / loopback port 18787", result.stdout)
        self.assertIn("choose Launch", result.stdout)
        for profile in self.state()["profiles"]:
            self.assertNotIn("releases/", json.dumps(profile))
            self.assertEqual(profile["command"], str(self.home / ".local/bin/pockymoe"))
            self.assertIn(str(data / "treer-attach.token"), profile["args"])
        self.assertIn("POCKYMOE_WEB_DIST_DIR=" + str(self.home / ".local/share/remote-codex/native/current/web"),
                      self.state()["apps"][0]["args"])
        self.assertTrue(data.is_dir())
        self.assertIn("POCKYMOE_DATABASE_PATH=" + str(data / "supervisor.sqlite"), self.state()["apps"][0]["args"])

    def test_reuse_and_idempotent_rerun_repairs_profiles_keeps_hub_port(self):
        self.installed()
        self.run_setup("--version", "0.13.2", "--hub-port", "18788")
        state = self.state()
        state["profiles"][0]["args"] = ["stale"]
        state["calls"] = []
        self.save(state)
        result = self.run_setup("--version", "0.13.2")
        state = self.state()
        self.assertEqual(len(state["apps"]), 1)
        self.assertEqual(len(state["profiles"]), 3)
        self.assertEqual(state["apps"][0]["port"], 18788)
        self.assertEqual(state["profiles"][0]["args"][:3], ["attach", "--provider", "codex"])
        self.assertTrue(all("http://127.0.0.1:18788" in p["args"] for p in state["profiles"]))
        self.assertEqual(sum(c[:4] == ["agent", "admin", "profile", "update"] for c in state["calls"]), 3)
        self.assertFalse(any("create" in c for c in state["calls"]))
        self.assertFalse((self.home / "installer-version").exists())
        self.assertIn("reusing installed", result.stdout)

    def test_version_mismatch_refuses_without_modifying_install_or_treer(self):
        binary = self.installed("0.13.1")
        before = binary.read_bytes()
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertIn("Settings -> Supervisor -> Update", result.stderr)
        self.assertEqual(binary.read_bytes(), before)
        self.assertEqual(self.state()["apps"], [])
        self.assertEqual(self.state()["profiles"], [])
        self.assertFalse((self.home / "installer-version").exists())

    def test_contract_refusal_before_install(self):
        bad_contracts = [None, {}, {"protocol": "treer.agent-interface/v2"},
                         {"protocol": "treer.agent-interface/v1", "features": []},
                         {"protocol": "treer.agent-interface/v1", "capabilities": [123], "features": []},
                         {"protocol": "treer.agent-interface/v1", "capabilities": [],
                          "features": ["agent.interface-provenance.v1"]}]
        for contract in bad_contracts:
            with self.subTest(contract=contract):
                state = self.state()
                state["contract"] = contract
                self.save(state)
                self.run_setup("--version", "0.13.2", success=False)
                self.assertEqual(self.state()["apps"], [])
                self.assertFalse((self.home / "installer-version").exists())

    def test_discovery_retry(self):
        self.installed()
        state = self.state()
        state["identityFailures"] = 2
        self.save(state)
        self.run_setup("--version", "0.13.2")
        self.assertEqual(self.state()["calls"].count(["whoami"]), 3)

    def test_first_free_port_skips_occupied_loopback(self):
        self.installed()
        with socket.socket() as listener:
            try:
                listener.bind(("127.0.0.1", 8787))
            except OSError:
                self.skipTest("Port 8787 is already occupied by a real service")
            listener.listen()
            expected = configure.free_port()
            self.assertGreater(expected, 8787)
            self.run_setup("--version", "0.13.2")
        self.assertEqual(self.state()["apps"][0]["port"], expected)

    def test_partial_failure_rerun_converges_without_duplicates(self):
        self.installed()
        state = self.state()
        state["failProfile"] = "Pockymoe Claude"
        self.save(state)
        self.run_setup("--version", "0.13.2", success=False)
        self.assertEqual(len(self.state()["apps"]), 1)
        self.assertEqual(len(self.state()["profiles"]), 1)
        self.run_setup("--version", "0.13.2")
        self.assertEqual(len(self.state()["apps"]), 1)
        self.assertEqual(len(self.state()["profiles"]), 3)

    def test_existing_hub_port_or_configuration_mismatch_refuses(self):
        self.installed()
        self.run_setup("--version", "0.13.2", "--hub-port", "18789")
        self.run_setup("--version", "0.13.2", "--hub-port", "18790", success=False)
        self.run_setup("--version", "0.13.2", "--data-dir", str(self.home / "different data"), success=False)
        self.assertEqual(len(self.state()["apps"]), 1)
        self.assertEqual(len(self.state()["profiles"]), 3)

    def test_version_and_port_arguments_are_strict(self):
        for args in [[], ["--version", "latest"], ["--version", "0.13.2", "--hub-port", "0"],
                     ["--version", "0.13.2", "--hub-port", "65536"]]:
            with self.subTest(args=args):
                self.run_setup(*args, success=False)
        self.assertEqual(self.state()["calls"], [])

    def test_missing_workload_identity_or_treer_fails_preflight(self):
        self.env.pop("TREER_AGENT_ID")
        self.run_setup("--version", "0.13.2", success=False)
        self.env["TREER_AGENT_ID"] = "installer-fixture"
        self.env["TREER_BIN"] = str(self.root / "missing-treer")
        self.run_setup("--version", "0.13.2", success=False)
        self.assertEqual(self.state()["calls"], [])

    def test_duplicate_app_names_refuse_instead_of_creating_another(self):
        self.installed()
        self.run_setup("--version", "0.13.2")
        state = self.state()
        state["apps"].append(dict(state["apps"][0], app_id="duplicate-hub"))
        state["calls"] = []
        self.save(state)
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertIn("Duplicate Pockymoe", result.stderr)
        self.assertFalse(any("create" in call or "update" in call for call in self.state()["calls"]))


if __name__ == "__main__":
    unittest.main()
