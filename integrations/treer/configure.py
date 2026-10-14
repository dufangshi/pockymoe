#!/usr/bin/env python3
"""Create Treer attach launch profiles and, when requested, a hub Managed App."""
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys


def commands(definitions, pockymoe, data_dir, hub_port, cwd, treer, create_hub):
    values = {"pockymoe": str(pockymoe), "dataDir": str(data_dir), "hubPort": str(hub_port),
              "nativeRoot": str(Path.home() / ".local/share/remote-codex/native")}
    result = []
    if create_hub:
        hub = definitions["hub"]
        result.append([treer, "app", "create", "--name", hub["name"], "--port", str(hub_port),
                       "--cwd", cwd, hub["command"], "--"] + [arg.format_map(values) for arg in hub["args"]])
    for profile in definitions["profiles"]:
        result.append([treer, "agent", "admin", "profile", "create", profile["name"],
                       "--description", profile["description"], "--cwd", cwd, str(pockymoe), "--"]
                      + profile["args"] + ["--hub", f"http://127.0.0.1:{hub_port}",
                                           "--hub-token-file", str(data_dir / "treer-attach.token")])
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hub-port", type=int, default=8787)
    parser.add_argument("--data-dir", type=Path, default=Path.home() / ".local/share/remote-codex/treer")
    parser.add_argument("--pockymoe", type=Path, default=Path.home() / ".local/bin/pockymoe")
    parser.add_argument("--cwd", default=".", help="Treer machine-relative profile cwd; Launch can override it")
    parser.add_argument("--create-hub", action="store_true", help="Also create and start the Managed App")
    parser.add_argument("--dry-run", action="store_true", help="Print commands without creating anything")
    args = parser.parse_args()
    if not 1 <= args.hub_port <= 65535:
        parser.error("hub port must be 1..65535")
    # Retain the lexical stable link instead of Path.resolve()'s versioned target.
    binary = args.pockymoe.expanduser().absolute()
    if "releases" in binary.parts or not binary.is_file():
        parser.error("pockymoe must be an installed stable executable, never releases/V")
    data = args.data_dir.expanduser().absolute()
    definitions = json.loads(Path(__file__).with_name("profiles.json").read_text())
    invocations = commands(definitions, binary, data, args.hub_port, args.cwd,
                           os.environ.get("TREER_BIN", "treer"), args.create_hub)
    for invocation in invocations:
        if args.dry_run:
            print(shlex.join(invocation))
        else:
            try:
                # Treer responses can include identity fields; do not forward them.
                subprocess.run(invocation, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, check=True, timeout=60)
            except (OSError, subprocess.SubprocessError):
                print("Treer configuration failed; inspect Apps/profiles before retrying (earlier creations remain).", file=sys.stderr)
                return 1
    if not args.dry_run:
        print("Pockymoe Treer profiles created." + (" Hub Managed App started." if args.create_hub else ""))
    return 0


if __name__ == "__main__":
    sys.exit(main())
