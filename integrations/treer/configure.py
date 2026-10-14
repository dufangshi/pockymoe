#!/usr/bin/env python3
"""Converge Treer attach launch profiles and an optional hub Managed App."""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import socket
import subprocess
import sys
import time
import unicodedata
import urllib.error
import urllib.request

import install

DEFAULT_DATA = Path.home() / ".local/share/remote-codex/treer"
HUB_HEALTH_TIMEOUT = 60
ADAPTER_TIMEOUT = 8 * 60
SUPERVISOR_TIMEOUT = 10 * 60
HUB_REQUEST_TIMEOUT = 30
HUB_POLL_INTERVAL = 2
HUB_PROGRESS_INTERVAL = 20


def safe_hub_text(value):
    """Hub diagnostics are non-secret; keep them bounded and terminal-safe."""
    return "".join(c for c in str(value) if not unicodedata.category(c).startswith("C"))[:400]


class NoHubRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("Hub redirected a loopback management request; check the local-mode App.")


def hub_request(port, path, body=None, timeout=HUB_REQUEST_TIMEOUT):
    # Bypass inherited proxy settings and redirects: management stays on loopback.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoHubRedirect())
    request = urllib.request.Request(f"http://127.0.0.1:{port}{path}",
                                    data=json.dumps(body).encode() if body is not None else None,
                                    headers={"Content-Type": "application/json"})
    try:
        with opener.open(request, timeout=timeout) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        message = f"Hub HTTP {error.code}"
        try:
            detail = json.load(error).get("message")
            if detail:
                message += ": " + safe_hub_text(detail)
        except (OSError, ValueError, AttributeError):
            pass
        raise ValueError(message) from None
    except (OSError, ValueError) as error:
        if isinstance(error, ValueError) and not isinstance(error, json.JSONDecodeError):
            raise
        raise ValueError("Hub management request failed; check the local-mode App and retry.") from None


def wait_for_hub(port):
    print(f"Pockymoe setup: waiting up to {HUB_HEALTH_TIMEOUT}s for hub health on 127.0.0.1:{port}.", flush=True)
    deadline = time.monotonic() + HUB_HEALTH_TIMEOUT
    next_progress = time.monotonic() + HUB_PROGRESS_INTERVAL
    while time.monotonic() < deadline:
        try:
            health = hub_request(port, "/healthz", timeout=min(5, max(0.001, deadline - time.monotonic())))
            if isinstance(health, dict) and health.get("status") == "ok":
                print("Pockymoe setup: hub health verified.", flush=True)
                return
        except ValueError:
            pass
        if time.monotonic() >= next_progress:
            print("Pockymoe setup: still waiting for hub health.", flush=True)
            next_progress = time.monotonic() + HUB_PROGRESS_INTERVAL
        time.sleep(min(HUB_POLL_INTERVAL, max(0, deadline - time.monotonic())))
    raise ValueError(f"Hub health timed out on 127.0.0.1:{port}; App/profiles remain. Check that the Treer App pockymoe is running in local mode, then rerun setup.")


def hub_harnesses(port):
    rows = hub_request(port, "/api/management/harnesses")
    if not isinstance(rows, list) or any(
            not isinstance(row, dict)
            or not isinstance(row.get("base"), dict)
            or (row.get("adapter") is not None and not isinstance(row["adapter"], dict))
            for row in rows):
        raise ValueError("Hub returned an invalid harness inventory.")
    return rows


def adapter_verified(job):
    return (isinstance(job, dict) and job.get("action") == "update"
            and job.get("component") == "adapter" and job.get("state") == "completed"
            and job.get("connectionVerified") is True)


def adapter_managed(adapter):
    return (isinstance(adapter, dict) and adapter.get("installed") is True
            and adapter.get("manager") == "pockymoe" and bool(adapter.get("path")))


def update_adapter(port, harness_id):
    print(f"Pockymoe setup: updating {harness_id} ACP adapter and verifying its connection (up to 8 min).", flush=True)
    deadline = time.monotonic() + ADAPTER_TIMEOUT
    hub_request(port, f"/api/management/harnesses/{harness_id}",
                {"action": "update", "component": "adapter"},
                timeout=min(HUB_REQUEST_TIMEOUT, max(0.001, deadline - time.monotonic())))
    next_progress = time.monotonic() + HUB_PROGRESS_INTERVAL
    while time.monotonic() < deadline:
        jobs = hub_request(port, "/api/management/jobs",
                           timeout=min(HUB_REQUEST_TIMEOUT, max(0.001, deadline - time.monotonic())))
        if not isinstance(jobs, dict):
            raise ValueError("Hub returned invalid management jobs.")
        job = jobs.get(harness_id)
        if isinstance(job, dict) and job.get("action") == "update" and job.get("component") == "adapter":
            if job.get("state") == "failed":
                raise ValueError(safe_hub_text(job.get("error") or "ACP adapter update failed."))
            if job.get("state") == "completed":
                if not adapter_verified(job):
                    raise ValueError("Adapter update completed without connectionVerified; ACP verification is required.")
                return
        if time.monotonic() >= next_progress:
            print(f"Pockymoe setup: waiting for {harness_id} ACP adapter installation/verification.", flush=True)
            next_progress = time.monotonic() + HUB_PROGRESS_INTERVAL
        time.sleep(min(HUB_POLL_INTERVAL, max(0, deadline - time.monotonic())))
    raise ValueError("ACP adapter installation/verification timed out after 8 min; the hub job may still be running.")


def verify_adapters(port):
    rows = hub_harnesses(port)
    failures = False
    for row in rows:
        harness_id = row.get("id")
        if row.get("transport") != "adapter" or harness_id not in ("codex", "claude"):
            continue
        base = row.get("base") or {}
        adapter = row.get("adapter") or {}
        # Current runtime DTOs omit installed=true on successful base inspection.
        if not base.get("installed", bool(base.get("path"))):
            print(f"Note: '{harness_id}' is not on PATH in the hub; install and sign in to it before launching Pockymoe {harness_id.title()}. ACP adapter skipped.", flush=True)
            adapter_summary(harness_id, adapter, "skipped: base CLI missing")
            continue
        failure = None
        try:
            if adapter_managed(adapter) and adapter_verified(row.get("job")):
                print(f"Pockymoe setup: {harness_id} ACP adapter already managed and verified; skipped.", flush=True)
            else:
                update_adapter(port, harness_id)
                refreshed = next((r for r in hub_harnesses(port) if r.get("id") == harness_id), {})
                adapter = refreshed.get("adapter") or {}
                if not adapter_managed(adapter):
                    raise ValueError("Update completed but the selected adapter is not a Pockymoe-managed installation.")
        except ValueError as error:
            failures = True
            failure = safe_hub_text(error)
            # Installation may have succeeded before ACP verification failed.
            try:
                refreshed = next((r for r in hub_harnesses(port) if r.get("id") == harness_id), {})
                adapter = refreshed.get("adapter") or adapter
            except ValueError:
                pass
        adapter_summary(harness_id, adapter, f"failed: {failure}" if failure else "verified")
    if failures:
        raise ValueError("ACP adapter setup failed; App/profiles remain. Use Pockymoe Settings -> Harnesses -> ACP adapter -> Install, or rerun setup.")


def adapter_summary(harness_id, adapter, result):
    print(f"ACP adapter {harness_id}: version {safe_hub_text(adapter.get('version') or 'unknown')}, "
          f"path {safe_hub_text(adapter.get('path') or 'missing')}; {result}", flush=True)


def commands(definitions, pockymoe, data_dir, hub_port, cwd, treer, create_hub):
    values = {"pockymoe": str(pockymoe), "dataDir": str(data_dir), "hubPort": str(hub_port),
              "nativeRoot": str(Path.home() / ".local/share/remote-codex/native")}
    result = []
    if create_hub:
        hub = definitions["hub"]
        result.append([treer, "app", "create", "--name", hub["name"], "--port", str(hub_port),
                       "--cwd", cwd, hub["command"], "--"]
                      + [arg.format_map(values) for arg in hub["args"]])
    for profile in definitions["profiles"]:
        result.append([treer, "agent", "admin", "profile", "create", profile["name"],
                       "--description", profile["description"], "--cwd", cwd, str(pockymoe), "--"]
                      + profile["args"] + ["--hub", f"http://127.0.0.1:{hub_port}",
                                           "--hub-token-file", str(data_dir / "treer-attach.token")])
    return result


def treer_run(treer, *args, read=False):
    # Never forward raw Treer output: it can contain identity/credential fields.
    try:
        result = subprocess.run([treer, *args], stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE if read else subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, check=True, timeout=60)
        return json.loads(result.stdout) if read else None
    except (OSError, ValueError, subprocess.SubprocessError):
        raise ValueError("Treer command failed; check access and the Treer Controller.") from None


def whoami(treer):
    delay = 0.1
    reported = False
    started = time.monotonic()
    while True:
        try:
            identity = treer_run(treer, "whoami", read=True)
        except ValueError:
            if not reported and time.monotonic() - started >= 10:
                print("Pockymoe setup: waiting for Treer to publish this Agent's identity.", flush=True)
                reported = True
            time.sleep(delay)
            delay = min(delay * 2, 30)
            continue
        if not isinstance(identity, dict) or not isinstance(identity.get("machine"), dict):
            raise ValueError("Treer whoami did not report a machine identity.")
        machine = identity["machine"]
        if not isinstance(machine.get("server_id"), str) or not machine["server_id"]:
            raise ValueError("Treer whoami did not report a machine server_id.")
        return machine


def check_contract(treer):
    contract = treer_run(treer, "interface", "contract", read=True)
    if not isinstance(contract, dict) or contract.get("protocol") != "treer.agent-interface/v1":
        raise ValueError("Treer must report AIS v1 (treer.agent-interface/v1). Upgrade Treer and retry.")
    for key in ("capabilities", "features"):
        values = contract.get(key)
        if not isinstance(values, list) or any(not isinstance(v, str) for v in values):
            raise ValueError("Treer AIS capabilities/features are unknown or invalid. Upgrade Treer and retry.")
    if not {"agent.interface-provenance.v1", "agent.prompt-delivery.v1"} <= set(contract["features"]):
        raise ValueError("Treer requires agent.interface-provenance.v1 and agent.prompt-delivery.v1. Upgrade Treer and retry.")


def named_entries(treer, args, key):
    response = treer_run(treer, *args, read=True)
    entries = response.get(key) if isinstance(response, dict) else None
    if not isinstance(entries, list) or any(not isinstance(e, dict) or not isinstance(e.get("name"), str)
                                            for e in entries):
        raise ValueError("Treer returned an invalid App/profile list; refusing to create duplicates.")
    return entries


def unique(entries, name):
    matches = [entry for entry in entries if entry["name"] == name]
    if len(matches) > 1:
        raise ValueError("Duplicate Pockymoe App/profile names exist; resolve them in Treer before retrying.")
    return matches[0] if matches else None


def free_port():
    for port in range(8787, 65536):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
            try:
                listener.bind(("127.0.0.1", port))
            except OSError:
                continue
            return port
    raise ValueError("No free loopback hub port is available from 8787 upward.")


def existing_hub_port(hub, hub_port, machine):
    if hub.get("server_id") != machine["server_id"]:
        raise ValueError("The Pockymoe App belongs to another machine; use that machine's Treer Agent.")
    port = hub.get("port")
    if type(port) is not int or not 1 <= port <= 65535:
        raise ValueError("Existing Pockymoe App has an invalid port.")
    if hub_port is not None and hub_port != port:
        raise ValueError("Existing Pockymoe App uses a different port; rerun with its port or omit --hub-port.")
    return port


def check_hub_command(hub, definitions, binary, data, port, cwd, treer):
    invocation = commands(definitions, binary, data, port, cwd, treer, True)[0]
    if (hub.get("command") != definitions["hub"]["command"] or hub.get("cwd") != cwd
            or hub.get("args") != invocation[invocation.index("--") + 1:]):
        raise ValueError("Existing Pockymoe App configuration differs; inspect its stable command/data directory in Treer before retrying.")


def converge(definitions, binary, data, hub_port, cwd, treer, create_hub, machine, dry_run=False):
    apps = named_entries(treer, ("app", "list"), "apps") if create_hub else []
    profiles = named_entries(treer, ("agent", "admin", "profile", "list"), "profiles")
    hub = unique(apps, definitions["hub"]["name"])
    existing = [unique(profiles, profile["name"]) for profile in definitions["profiles"]]
    if hub:
        hub_port = existing_hub_port(hub, hub_port, machine)
    elif hub_port is None:
        hub_port = free_port() if create_hub else 8787
    invocations = commands(definitions, binary, data, hub_port, cwd, treer, create_hub)
    if create_hub:
        invocation = invocations.pop(0)
        if hub:
            check_hub_command(hub, definitions, binary, data, hub_port, cwd, treer)
            print(f"Pockymoe setup: reusing hub App pockymoe on loopback port {hub_port}.", flush=True)
        else:
            print(f"Pockymoe setup: creating hub App pockymoe on loopback port {hub_port}.", flush=True)
            if dry_run:
                print(shlex.join(invocation))
            else:
                data.mkdir(parents=True, exist_ok=True, mode=0o700)
                treer_run(*invocation)
    for definition, current, invocation in zip(definitions["profiles"], existing, invocations):
        if current:
            profile_id = current.get("profile_id")
            if not isinstance(profile_id, str) or not profile_id:
                raise ValueError("Existing Pockymoe profile has no stable ID.")
            args = invocation[invocation.index("--") + 1:]
            invocation = [treer, "agent", "admin", "profile", "update", profile_id,
                          "--description", definition["description"], "--cwd", cwd,
                          "--command", str(binary)] + [f"--arg={arg}" for arg in args]
        print(f"Pockymoe setup: {'updating' if current else 'creating'} profile {definition['name']}.", flush=True)
        if dry_run:
            print(shlex.join(invocation))
        else:
            treer_run(*invocation)
    return hub_port


def installed_runtime(binary, selected, allow_install=True):
    if os.path.lexists(binary):
        try:
            checked = subprocess.run([str(binary), "version"], capture_output=True, timeout=10, check=True)
            found = install.version(checked.stdout.decode().strip())
        except (OSError, ValueError, subprocess.SubprocessError):
            raise ValueError("Existing Pockymoe CLI could not report its version; inspect it before retrying.") from None
        if not allow_install and found != selected:
            raise ValueError("Installer did not create the requested Pockymoe version; inspect the stable CLI before retrying.")
        if tuple(map(int, found.split("."))) > tuple(map(int, selected.split("."))):
            raise ValueError(f"Installed Pockymoe {found} is newer than requested {selected}; setup never downgrades. Rerun with the installed version or a newer release.")
        if found == selected:
            print(f"Pockymoe setup: reusing installed Pockymoe {selected}.", flush=True)
        return found
    else:
        if not allow_install:
            raise ValueError("Installer did not create the stable Pockymoe CLI.")
        print(f"Pockymoe setup: installing pinned Pockymoe {selected} (SHA256SUMS verified).", flush=True)
        subprocess.run([sys.executable, str(Path(__file__).with_name("install.py")), "--version", selected],
                       stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       check=True)
        # Verify the stable path after installation too.
        return installed_runtime(binary, selected, allow_install=False)


def supervisor_snapshot(port, path="/api/management/supervisor", body=None, timeout=HUB_REQUEST_TIMEOUT):
    value = hub_request(port, path, body, timeout)
    if (not isinstance(value, dict) or not isinstance(value.get("runningVersion"), str)
            or (value.get("job") is not None and not isinstance(value["job"], dict))):
        raise ValueError("Hub returned an invalid Supervisor status; inspect Settings -> Supervisor before retrying.")
    return value


def check_update_job(status, selected):
    job = status.get("job") or {}
    if not isinstance(job, dict):
        raise ValueError("Hub returned an invalid Supervisor update job.")
    target = job.get("targetVersion")
    if target is not None and target != selected:
        raise ValueError(f"Hub update target is {safe_hub_text(target)}, not requested {selected}; inspect Settings -> Supervisor. Setup cannot pin this API's latest-only update.")
    if job.get("phase") in ("failed", "rolled-back", "rollback-failed"):
        raise ValueError("Hub update failed: " + safe_hub_text(job.get("error") or "Unknown update failure.")
                         + " Check Settings -> Supervisor, then rerun setup.")
    return job


def upgrade_runtime(definitions, binary, data, hub_port, treer, machine, found, selected):
    hub = unique(named_entries(treer, ("app", "list"), "apps"), definitions["hub"]["name"])
    if not hub:
        raise ValueError("An older Pockymoe is installed, but no setup-created hub App exists. Start the existing setup-created App, or use Settings -> Supervisor -> Update; setup never replaces an existing install.")
    port = existing_hub_port(hub, hub_port, machine)
    check_hub_command(hub, definitions, binary, data, port, ".", treer)
    print(f"Pockymoe setup: checking running hub on 127.0.0.1:{port} before upgrading {found} to {selected}.", flush=True)
    try:
        status = supervisor_snapshot(port)
    except ValueError as error:
        raise ValueError(f"No reachable running setup-created hub on 127.0.0.1:{port}: {safe_hub_text(error)} Start App pockymoe in Treer and rerun setup; the installed runtime was preserved.") from None
    if status.get("restartStrategy") != "host-managed":
        raise ValueError("The running hub must use the host-managed restart strategy; inspect App pockymoe and Settings -> Supervisor before retrying.")
    if status["runningVersion"] != found or status.get("installedVersion") != found:
        raise ValueError("Hub running/installed versions differ from the stable CLI; inspect Settings -> Supervisor and let any current update finish before rerunning setup.")
    if (status.get("job") or {}).get("phase") in ("scheduled", "preparing", "installing", "restarting", "verifying"):
        raise ValueError("A hub update is already running; let it finish and rerun setup.")
    checked = supervisor_snapshot(port, "/api/management/supervisor/check", {})
    # The current endpoint has no target-version input and always resolves latest.
    if checked.get("latestVersion") != selected:
        raise ValueError(f"The hub update API supports latest only; latest is {safe_hub_text(checked.get('latestVersion') or 'unknown')}, requested {selected}. No update requested. Rerun with the exact latest release if desired.")
    if checked.get("canUpdate") is not True:
        raise ValueError("Hub cannot update: " + safe_hub_text(checked.get("reason") or "Inspect Settings -> Supervisor before retrying."))
    print(f"Pockymoe setup: requesting hub-managed update to {selected} (up to 10 min); Treer will relaunch the hub.", flush=True)
    deadline = time.monotonic() + SUPERVISOR_TIMEOUT
    accepted = supervisor_snapshot(port, "/api/management/supervisor/update", {},
                                   timeout=min(HUB_REQUEST_TIMEOUT, SUPERVISOR_TIMEOUT))
    check_update_job(accepted, selected)
    next_progress = time.monotonic() + HUB_PROGRESS_INTERVAL
    while time.monotonic() < deadline:
        try:
            status = supervisor_snapshot(port, timeout=min(5, max(0.001, deadline - time.monotonic())))
        except ValueError:
            # A disconnect while Treer relaunches the App is expected.
            status = None
        if status is not None:
            job = check_update_job(status, selected)
            if status["runningVersion"] == selected and job.get("phase") in (None, "completed"):
                print(f"Pockymoe setup: running hub verified at version {selected}.", flush=True)
                return
        if time.monotonic() >= next_progress:
            print(f"Pockymoe setup: waiting for Treer hub restart and running version {selected}.", flush=True)
            next_progress = time.monotonic() + HUB_PROGRESS_INTERVAL
        time.sleep(min(HUB_POLL_INTERVAL, max(0, deadline - time.monotonic())))
    raise ValueError(f"Hub upgrade timed out waiting for running version {selected}; the hub job may still be running. Check Treer App pockymoe and Settings -> Supervisor, then rerun setup. Setup did not replace the runtime directly.")


def main():
    setup = sys.argv[1:2] == ["--setup"]
    parser = argparse.ArgumentParser(prog="treer-setup.sh" if setup else None, description=__doc__)
    if setup:
        parser.add_argument("--version", required=True, type=install.version, help="Exact stable runtime release")
    parser.add_argument("--hub-port", type=int, default=None if setup else 8787)
    parser.add_argument("--data-dir", type=Path, default=DEFAULT_DATA)
    if not setup:
        parser.add_argument("--pockymoe", type=Path, default=Path.home() / ".local/bin/pockymoe")
        parser.add_argument("--cwd", default=".", help="Treer machine-relative profile cwd; Launch can override it")
        parser.add_argument("--create-hub", action="store_true", help="Also create/start or reuse the Managed App")
        parser.add_argument("--dry-run", action="store_true", help="Read existing resources and print planned writes")
    args = parser.parse_args(sys.argv[2:] if setup else None)
    if args.hub_port is not None and not 1 <= args.hub_port <= 65535:
        parser.error("hub port must be 1..65535")
    binary = (Path.home() / ".local/bin/pockymoe" if setup else args.pockymoe).expanduser().absolute()
    data = args.data_dir.expanduser().absolute()
    treer = os.environ.get("TREER_BIN", "treer")
    try:
        if not shutil.which(treer):
            raise ValueError("treer is required on PATH (or set TREER_BIN).")
        if not os.environ.get("TREER_AGENT_ID"):
            raise ValueError("Run setup inside a Treer installer or terminal Agent (TREER_AGENT_ID required).")
        if setup:
            install.asset_name()  # Same supported platforms as the pinned installer.
            print("Pockymoe setup: platform, Python 3.11+ and Treer executable verified.", flush=True)
        elif "releases" in binary.parts or not binary.is_file():
            raise ValueError("pockymoe must be an installed stable executable, never releases/V.")
        machine = whoami(treer)
        print("Pockymoe setup: Treer Agent identity discovered.", flush=True)
        check_contract(treer)
        print("Pockymoe setup: AIS v1 provenance and prompt-delivery contract verified.", flush=True)
        definitions = json.loads(Path(__file__).with_name("profiles.json").read_text())
        if setup:
            found = installed_runtime(binary, args.version)
            if found != args.version:
                upgrade_runtime(definitions, binary, data, args.hub_port, treer, machine, found, args.version)
        if "releases" in binary.parts or not binary.is_file():
            raise ValueError("pockymoe must be an installed stable executable, never releases/V.")
        port = converge(definitions, binary, data, args.hub_port, "." if setup else args.cwd, treer,
                        setup or args.create_hub, machine, False if setup else args.dry_run)
        if setup:
            print("Pockymoe setup: App/profiles configured; checking ACP adapters next.", flush=True)
            missing_base_notes(definitions)
            wait_for_hub(port)
            verify_adapters(port)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        # Only our own ValueErrors are safe; OS/subprocess diagnostics may contain secrets.
        message = str(error) if isinstance(error, ValueError) else "Installation/configuration failed; inspect the pinned release, installation layout and Treer access before retrying."
        print(f"Pockymoe setup failed: {message}", file=sys.stderr)
        return 1
    print(f"Pockymoe ready: App pockymoe / loopback port {port}." if setup or args.create_hub
          else f"Pockymoe profiles ready: hub loopback port {port}.")
    print("Profiles: " + ", ".join(profile["name"] for profile in definitions["profiles"]))
    print("In Treer, choose Launch, select a Pockymoe profile and set your project working directory.")
    if not setup:
        missing_base_notes(definitions)
    return 0


def missing_base_notes(definitions):
    # The hub refuses to create a thread whose base agent CLI is not on its PATH.
    for profile in definitions["profiles"]:
        base = base_agent(profile["args"])
        if base and shutil.which(base) is None:
            print(f"Note: '{base}' is not on PATH here; install and sign in to it before "
                  f"launching {profile['name']}.")


def base_agent(args):
    """Base CLI the hub requires for an attach profile's provider."""
    if "--agent" in args:
        return args[args.index("--agent") + 1]
    if "--provider" in args:
        provider = args[args.index("--provider") + 1]
        return provider if provider != "acp" else None
    return None


if __name__ == "__main__":
    sys.exit(main())
