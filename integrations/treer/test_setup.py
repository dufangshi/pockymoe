"""Offline POSIX setup tests; every command/state lives in a temporary fake home."""
from contextlib import redirect_stdout
import io
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import bundle_setup
import configure
from fake_hub import FakeHub

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
        configuration = self.bundle / "configure.py"
        configuration.write_text(configuration.read_text()
                                 .replace("HUB_HEALTH_TIMEOUT = 60", "HUB_HEALTH_TIMEOUT = 0.2")
                                 .replace("ADAPTER_TIMEOUT = 8 * 60", "ADAPTER_TIMEOUT = 0.25")
                                 .replace("SUPERVISOR_TIMEOUT = 10 * 60", "SUPERVISOR_TIMEOUT = 0.25")
                                 .replace("HUB_POLL_INTERVAL = 2", "HUB_POLL_INTERVAL = 0.01")
                                 .replace("HUB_PROGRESS_INTERVAL = 20", "HUB_PROGRESS_INTERVAL = 0.05"))
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
        self.hub = FakeHub()
        self.addCleanup(self.hub.close)

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
        if "--version" in args and "--hub-port" not in args and not self.state()["apps"]:
            args = (*args, "--hub-port", str(self.hub.port))
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
        result = self.run_setup("--version", "0.13.2", "--hub-port", str(self.hub.port), "--data-dir", str(data))
        self.assertEqual((self.home / "installer-version").read_text(), "0.13.2")
        self.assertEqual(len(self.state()["apps"]), 1)
        self.assertEqual(len(self.state()["profiles"]), 3)
        self.assertIn(f"App pockymoe / loopback port {self.hub.port}", result.stdout)
        self.assertIn("choose Launch", result.stdout)
        for profile in self.state()["profiles"]:
            self.assertNotIn("releases/", json.dumps(profile))
            self.assertEqual(profile["command"], str(self.home / ".local/bin/pockymoe"))
            self.assertIn(str(data / "treer-attach.token"), profile["args"])
        self.assertIn("POCKYMOE_WEB_DIST_DIR=" + str(self.home / ".local/share/remote-codex/native/current/web"),
                      self.state()["apps"][0]["args"])
        self.assertTrue(data.is_dir())
        self.assertIn("POCKYMOE_DATABASE_PATH=" + str(data / "supervisor.sqlite"), self.state()["apps"][0]["args"])

    def test_summary_names_missing_base_agents_only(self):
        self.installed()
        claude = self.bin_dir / "claude"
        claude.write_text("#!/bin/sh\n")
        claude.chmod(0o700)
        self.env["PATH"] = os.pathsep.join([str(self.bin_dir), "/usr/bin", "/bin"])
        row = self.hub.row("claude")
        row["base"] = {"path": str(claude)}
        row["adapter"] = self.hub.managed_adapter("claude")
        row["job"] = {"state": "completed", "action": "update", "component": "adapter",
                      "connectionVerified": True}
        result = self.run_setup("--version", "0.13.2")
        self.assertIn("Note: 'codex' is not on PATH", result.stdout)
        self.assertIn("Note: 'grok' is not on PATH", result.stdout)
        self.assertNotIn("'claude' is not on PATH", result.stdout)

    def test_reuse_and_idempotent_rerun_repairs_profiles_keeps_hub_port(self):
        self.installed()
        self.run_setup("--version", "0.13.2")
        state = self.state()
        state["profiles"][0]["args"] = ["stale"]
        state["calls"] = []
        self.save(state)
        result = self.run_setup("--version", "0.13.2")
        state = self.state()
        self.assertEqual(len(state["apps"]), 1)
        self.assertEqual(len(state["profiles"]), 3)
        self.assertEqual(state["apps"][0]["port"], self.hub.port)
        self.assertEqual(state["profiles"][0]["args"][:3], ["attach", "--provider", "codex"])
        self.assertTrue(all(f"http://127.0.0.1:{self.hub.port}" in p["args"] for p in state["profiles"]))
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

    def existing_upgrade_hub(self, version="0.13.1"):
        self.installed(version)
        definitions = json.loads((SOURCE / "profiles.json").read_text())
        # Generate the same saved App command while retaining this fake home's paths.
        with patch.object(Path, "home", return_value=self.home):
            invocation = configure.commands(definitions, self.home / ".local/bin/pockymoe",
                                            self.home / ".local/share/remote-codex/treer",
                                            self.hub.port, ".", "treer", True)[0]
        state = self.state()
        state["apps"] = [{"app_id": "hub-id", "name": "pockymoe", "server_id": "test-machine",
                          "port": self.hub.port, "cwd": ".", "command": "/usr/bin/env",
                          "args": invocation[invocation.index("--") + 1:]}]
        self.save(state)
        self.hub.supervisor.update(runningVersion=version, installedVersion=version)

    def supervisor_updates(self):
        return [request for request in self.hub.requests
                if request[:2] == ("POST", "/api/management/supervisor/update")]

    def test_upgrade_success_waits_for_running_version_then_converges_and_verifies_adapters(self):
        self.existing_upgrade_hub()
        self.hub.supervisor_polls = [
            {"job": {"phase": "installing", "targetVersion": "0.13.2"}},
            503, 503, 503, 503,  # Treer host-managed restart disconnect.
            {"runningVersion": "0.13.2", "installedVersion": "0.13.2",
             "job": {"phase": "verifying", "targetVersion": "0.13.2"}},
            {"job": {"phase": "completed", "targetVersion": "0.13.2"}}]
        self.hub.on_supervisor_completed = self.installed
        self.hub.row("codex")["base"] = {"path": "/fixture/bin/codex"}
        result = self.run_setup("--version", "0.13.2")
        self.assertEqual(self.supervisor_updates(), [("POST", "/api/management/supervisor/update", {})])
        self.assertIn(("POST", "/api/management/supervisor/check", {}), self.hub.requests)
        self.assertIn("waiting for Treer hub restart and running version 0.13.2", result.stdout)
        self.assertLess(result.stdout.index("running hub verified at version 0.13.2"),
                        result.stdout.index("creating profile Pockymoe Codex"))
        self.assertLess(result.stdout.index("running hub verified at version 0.13.2"),
                        result.stdout.index("updating codex ACP adapter"))
        self.assertIn("ACP adapter codex: version 1.2.3", result.stdout)
        self.assertIn("Pockymoe ready:", result.stdout)
        self.assertEqual(len(self.state()["apps"]), 1)
        self.assertEqual(len(self.state()["profiles"]), 3)
        self.assertFalse(any(call[:2] in (["app", "restart"], ["agent", "restart"])
                             for call in self.state()["calls"]))
        self.assertFalse((self.home / "installer-version").exists())
        self.hub.requests.clear()
        self.run_setup("--version", "0.13.2")
        self.assertEqual(self.supervisor_updates(), [])
        self.assertEqual(self.adapter_updates(), [])

    def test_upgrade_latest_mismatch_refuses_without_mutations(self):
        self.existing_upgrade_hub()
        binary = self.home / ".local/bin/pockymoe"
        before = binary.read_bytes()
        self.hub.latest_version = "0.13.3"
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertIn("API supports latest only; latest is 0.13.3, requested 0.13.2", result.stderr)
        self.assertIn("No update requested", result.stderr)
        self.assertEqual(self.supervisor_updates(), [])
        self.assertEqual(self.state()["profiles"], [])
        self.assertEqual(binary.read_bytes(), before)

    def test_upgrade_timeout_preserves_hub_and_profiles(self):
        self.existing_upgrade_hub()
        before = self.state()
        binary = self.home / ".local/bin/pockymoe"
        contents = binary.read_bytes()
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertIn("Hub upgrade timed out waiting for running version 0.13.2", result.stderr)
        self.assertIn("hub job may still be running", result.stderr)
        self.assertIn("Settings -> Supervisor", result.stderr)
        self.assertEqual(len(self.supervisor_updates()), 1)
        self.assertEqual(self.state()["apps"], before["apps"])
        self.assertEqual(self.state()["profiles"], before["profiles"])
        self.assertEqual(binary.read_bytes(), contents)
        self.assertEqual(self.adapter_updates(), [])

    def test_downgrade_refusal_compares_numeric_versions(self):
        self.existing_upgrade_hub("0.13.10")
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertIn("Installed Pockymoe 0.13.10 is newer than requested 0.13.2", result.stderr)
        self.assertIn("never downgrades", result.stderr)
        self.assertEqual(self.hub.requests, [])
        self.assertEqual(self.state()["profiles"], [])

    def test_upgrade_refuses_when_existing_app_is_not_running(self):
        self.existing_upgrade_hub()
        self.hub.supervisor_available = False
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertIn("No reachable running setup-created hub", result.stderr)
        self.assertIn("Start App pockymoe in Treer and rerun setup", result.stderr)
        self.assertEqual(self.supervisor_updates(), [])
        self.assertEqual(self.state()["profiles"], [])

    def test_upgrade_refuses_app_with_wrong_stable_configuration(self):
        self.existing_upgrade_hub()
        state = self.state()
        state["apps"][0]["args"][-2] = "/native/releases/0.13.1/remote-codex"
        self.save(state)
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertIn("App configuration differs", result.stderr)
        self.assertEqual(self.hub.requests, [])
        self.assertEqual(self.state()["profiles"], [])

    def test_upgrade_failure_is_reported_and_sanitized(self):
        self.existing_upgrade_hub()
        self.hub.supervisor_polls = [{"job": {"phase": "failed", "targetVersion": "0.13.2",
                                               "error": "Download failed\x1b\r\n" + "x" * 1000}}]
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertIn("Hub update failed: Download failed", result.stderr)
        self.assertNotIn("\x1b", result.stdout + result.stderr)
        self.assertNotIn("x" * 401, result.stderr)
        self.assertEqual(self.state()["profiles"], [])

    def test_upgrade_detects_latest_changed_after_check(self):
        self.existing_upgrade_hub()
        self.hub.supervisor_polls = [{"job": {"phase": "installing", "targetVersion": "0.13.3"}}]
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertIn("Hub update target is 0.13.3, not requested 0.13.2", result.stderr)
        self.assertEqual(self.state()["profiles"], [])
        self.assertEqual(self.adapter_updates(), [])

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
            definitions = json.loads((SOURCE / "profiles.json").read_text())
            output = io.StringIO()
            with patch.object(configure, "named_entries", return_value=[]), redirect_stdout(output):
                selected = configure.converge(definitions, self.home / ".local/bin/pockymoe",
                                             self.home / "data", None, ".", "treer", True,
                                             {"server_id": "test-machine"}, dry_run=True)
            self.assertEqual(selected, expected)
            self.assertIn(f"--port {expected}", output.getvalue())
        self.assertEqual(self.state()["apps"], [])

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
        self.run_setup("--version", "0.13.2")
        other_port = self.hub.port - 1 if self.hub.port > 1 else self.hub.port + 1
        self.run_setup("--version", "0.13.2", "--hub-port", str(other_port), success=False)
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

    def adapter_updates(self):
        return [request for request in self.hub.requests
                if request[0] == "POST" and request[1].startswith("/api/management/harnesses/")]

    def test_missing_adapter_updates_and_verifies_before_ready(self):
        # Match the current runtime base DTO: successful inspection has path, no installed flag.
        self.hub.row("codex")["base"] = {"path": "/fixture/bin/codex"}
        self.hub.health_failures = 2
        self.hub.running_polls = 6
        result = self.run_setup("--version", "0.13.2")
        self.assertEqual(self.adapter_updates(), [
            ("POST", "/api/management/harnesses/codex", {"action": "update", "component": "adapter"})])
        self.assertIn("waiting for codex ACP adapter installation/verification", result.stdout)
        self.assertIn("hub health verified", result.stdout)
        self.assertIn("version 1.2.3, path /fixture/.local/share/remote-codex/adapters/installations/", result.stdout)
        self.assertIn("; verified", result.stdout)
        self.assertLess(result.stdout.index("; verified"), result.stdout.index("Pockymoe ready:"))
        self.assertGreaterEqual(sum(path == "/api/management/jobs" for _, path, _ in self.hub.requests), 2)
        self.hub.requests.clear()
        rerun = self.run_setup("--version", "0.13.2")
        self.assertEqual(self.adapter_updates(), [])
        self.assertIn("already managed and verified; skipped", rerun.stdout)
        self.assertEqual(len(self.state()["apps"]), 1)
        self.assertEqual(len(self.state()["profiles"]), 3)

    def test_external_adapter_is_replaced_even_with_successful_old_job(self):
        row = self.hub.row("claude")
        row["base"] = {"installed": True, "path": "/fixture/bin/claude"}
        row["adapter"] = {"installed": True, "manager": "npm", "path": "/external/claude-agent-acp",
                          "version": "0.0.1"}
        row["job"] = {"state": "completed", "action": "update", "component": "adapter",
                      "connectionVerified": True}
        result = self.run_setup("--version", "0.13.2")
        self.assertEqual(len(self.adapter_updates()), 1)
        self.assertIn("/harnesses/claude", self.adapter_updates()[0][1])
        self.assertIn("ACP adapter claude: version 1.2.3", result.stdout)
        self.assertNotIn("/external/claude-agent-acp; verified", result.stdout)

    def test_managed_and_verified_adapter_skips_update(self):
        row = self.hub.row("codex")
        row["base"] = {"path": "/fixture/bin/codex"}
        row["adapter"] = self.hub.managed_adapter("codex")
        row["job"] = {"state": "completed", "action": "update", "component": "adapter",
                      "connectionVerified": True}
        result = self.run_setup("--version", "0.13.2")
        self.assertEqual(self.adapter_updates(), [])
        self.assertIn("already managed and verified; skipped", result.stdout)
        self.assertIn("ACP adapter codex: version 1.2.3", result.stdout)

    def test_managed_adapter_without_successful_last_verification_updates(self):
        row = self.hub.row("codex")
        row["base"] = {"path": "/fixture/bin/codex"}
        row["adapter"] = self.hub.managed_adapter("codex")
        for job in (None, {"state": "failed", "action": "update", "component": "adapter"},
                    {"state": "completed", "action": "update", "component": "base"}):
            with self.subTest(job=job):
                row["job"] = job
                self.hub.requests.clear()
                self.run_setup("--version", "0.13.2")
                self.assertEqual(len(self.adapter_updates()), 1)

    def test_failed_adapter_leaves_profiles_and_continues_other_harnesses(self):
        for agent in ("codex", "claude"):
            self.hub.row(agent)["base"] = {"path": f"/fixture/bin/{agent}"}
        self.hub.job_results["codex"] = {
            "state": "failed", "action": "update", "component": "adapter",
            "error": "ACP initialization failed\x1b\r\n\x00" + "x" * 1000}
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertEqual(len(self.state()["apps"]), 1)
        self.assertEqual(len(self.state()["profiles"]), 3)
        self.assertEqual(len(self.adapter_updates()), 2)
        self.assertIn("failed: ACP initialization failed", result.stdout)
        self.assertNotIn("x" * 401, result.stdout)
        self.assertNotIn("\x1b", result.stdout + result.stderr)
        self.assertNotIn("\x00", result.stdout + result.stderr)
        self.assertIn("ACP adapter claude: version 1.2.3", result.stdout)
        self.assertIn("Settings -> Harnesses -> ACP adapter -> Install", result.stderr)
        self.assertIn("App/profiles remain", result.stderr)
        self.assertNotIn("Pockymoe ready:", result.stdout)
        del self.hub.job_results["codex"]
        self.hub.requests.clear()
        self.run_setup("--version", "0.13.2")
        self.assertEqual(len(self.adapter_updates()), 1)
        self.assertEqual(len(self.state()["profiles"]), 3)

    def test_completed_without_connection_verified_fails(self):
        self.hub.row("codex")["base"] = {"path": "/fixture/bin/codex"}
        self.hub.job_results["codex"] = {"state": "completed", "action": "update", "component": "adapter",
                                         "connectionVerified": False}
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertIn("completed without connectionVerified", result.stdout)
        self.assertEqual(len(self.state()["profiles"]), 3)

    def test_missing_base_skips_adapter_and_keeps_note(self):
        result = self.run_setup("--version", "0.13.2")
        self.assertEqual(self.adapter_updates(), [])
        self.assertIn("'codex' is not on PATH in the hub", result.stdout)
        self.assertIn("'claude' is not on PATH in the hub", result.stdout)
        self.assertIn("ACP adapter skipped", result.stdout)

    def test_hub_health_timeout_is_bounded_and_keeps_profiles(self):
        self.hub.healthy = False
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertIn(f"waiting up to 0.2s for hub health on 127.0.0.1:{self.hub.port}", result.stdout)
        self.assertIn("still waiting for hub health", result.stdout)
        self.assertIn(f"Hub health timed out on 127.0.0.1:{self.hub.port}", result.stderr)
        self.assertIn("Treer App pockymoe is running in local mode", result.stderr)
        self.assertEqual(len(self.state()["profiles"]), 3)
        self.assertEqual(self.adapter_updates(), [])

    def test_adapter_job_timeout_is_bounded(self):
        self.hub.row("codex")["base"] = {"path": "/fixture/bin/codex"}
        self.hub.running_polls = 1000
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertIn("installation/verification timed out", result.stdout)
        self.assertIn("hub job may still be running", result.stdout)
        self.assertEqual(len(self.state()["profiles"]), 3)

    def test_hub_http_error_is_reported_safely(self):
        self.hub.row("codex")["base"] = {"path": "/fixture/bin/codex"}
        self.hub.post_error = "This harness is busy.\x1b\r\nWait for turns to finish."
        result = self.run_setup("--version", "0.13.2", success=False)
        self.assertIn("Hub HTTP 409: This harness is busy.Wait for turns to finish.", result.stdout)
        self.assertNotIn("\x1b", result.stdout)


if __name__ == "__main__":
    unittest.main()
