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

import install

DEFAULT_DATA = Path.home() / ".local/share/remote-codex/treer"


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


def converge(definitions, binary, data, hub_port, cwd, treer, create_hub, machine, dry_run=False):
    apps = named_entries(treer, ("app", "list"), "apps") if create_hub else []
    profiles = named_entries(treer, ("agent", "admin", "profile", "list"), "profiles")
    hub = unique(apps, definitions["hub"]["name"])
    existing = [unique(profiles, profile["name"]) for profile in definitions["profiles"]]
    if hub:
        if hub.get("server_id") != machine["server_id"]:
            raise ValueError("The Pockymoe App belongs to another machine; use that machine's Treer Agent.")
        port = hub.get("port")
        if type(port) is not int or not 1 <= port <= 65535:
            raise ValueError("Existing Pockymoe App has an invalid port.")
        if hub_port is not None and hub_port != port:
            raise ValueError("Existing Pockymoe App uses a different port; rerun with its port or omit --hub-port.")
        hub_port = port
    elif hub_port is None:
        hub_port = free_port() if create_hub else 8787
    invocations = commands(definitions, binary, data, hub_port, cwd, treer, create_hub)
    if create_hub:
        invocation = invocations.pop(0)
        if hub:
            split = invocation.index("--")
            if (hub.get("command") != definitions["hub"]["command"] or hub.get("cwd") != cwd
                    or hub.get("args") != invocation[split + 1:]):
                raise ValueError("Existing Pockymoe App configuration differs; inspect its stable command/data directory in Treer before retrying.")
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
            found = checked.stdout.decode().strip()
        except (OSError, UnicodeError, subprocess.SubprocessError):
            raise ValueError("Existing Pockymoe CLI could not report its version; inspect it before retrying.") from None
        if found != selected:
            raise ValueError("A different Pockymoe version is installed. Update it through Pockymoe Settings -> Supervisor -> Update, then rerun; setup never replaces an existing install.")
        print(f"Pockymoe setup: reusing installed Pockymoe {selected}.", flush=True)
    else:
        if not allow_install:
            raise ValueError("Installer did not create the stable Pockymoe CLI.")
        print(f"Pockymoe setup: installing pinned Pockymoe {selected} (SHA256SUMS verified).", flush=True)
        subprocess.run([sys.executable, str(Path(__file__).with_name("install.py")), "--version", selected],
                       stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       check=True)
        # Verify the stable path after installation too.
        installed_runtime(binary, selected, allow_install=False)


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
        if setup:
            installed_runtime(binary, args.version)
        if "releases" in binary.parts or not binary.is_file():
            raise ValueError("pockymoe must be an installed stable executable, never releases/V.")
        definitions = json.loads(Path(__file__).with_name("profiles.json").read_text())
        port = converge(definitions, binary, data, args.hub_port, "." if setup else args.cwd, treer,
                        setup or args.create_hub, machine, False if setup else args.dry_run)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        # Only our own ValueErrors are safe; OS/subprocess diagnostics may contain secrets.
        message = str(error) if isinstance(error, ValueError) else "Installation/configuration failed; inspect the pinned release, installation layout and Treer access before retrying."
        print(f"Pockymoe setup failed: {message}", file=sys.stderr)
        return 1
    print(f"Pockymoe ready: App pockymoe / loopback port {port}." if setup or args.create_hub
          else f"Pockymoe profiles ready: hub loopback port {port}.")
    print("Profiles: " + ", ".join(profile["name"] for profile in definitions["profiles"]))
    print("In Treer, choose Launch, select a Pockymoe profile and set your project working directory.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
