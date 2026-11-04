#!/usr/bin/env python3
"""Test affected workspace packages from a trusted release to the final SHA."""
import argparse
import json
import os
from pathlib import Path
import subprocess


def affected(paths, packages, root):
    names = {package["name"] for package in packages}
    selected = set()
    for path in paths:
        owners = [p["name"] for p in packages if path.startswith(str(Path(p["manifest_path"]).parent.relative_to(root)) + "/")]
        if owners:
            selected.update(owners)
        elif path.startswith("migrations/"):
            selected.update(names & {"pockymoe-supervisor", "pockymoe-relay"})
        elif path in {"Cargo.toml", "Cargo.lock"} or path.startswith((".cargo/", "rust-toolchain")):
            selected.update(names)
        elif not (path.startswith(("docs/", ".agents/", ".github/", "scripts/", "apps/", "packages/", "themes/", "npm/", "e2e/", "integrations/")) or path.endswith(".md") or path in {"AGENTS.md", "CLAUDE.md", ".gitignore", "package.json", "pnpm-lock.yaml", "pnpm-workspace.yaml", "thread-ui.sha", "tsconfig.base.json"}):
            selected.update(names)
    # Test consumers as well as changed packages; migrations/auth/update never
    # switch to --workspace. They use the same dependency closure as other code.
    while True:
        consumers = {p["name"] for p in packages if any(d["name"] in selected for d in p["dependencies"])}
        expanded = selected | consumers
        if expanded == selected:
            return sorted(selected)
        selected = expanded


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--validation", choices=["auto", "full"], default="auto")
    parser.add_argument("--plan-only", action="store_true")
    args = parser.parse_args()
    root = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True, timeout=30).strip())
    subprocess.run(["git", "merge-base", "--is-ancestor", args.base, args.head], check=True, timeout=30)
    paths = subprocess.check_output(["git", "diff", "--name-only", "--no-renames", args.base, args.head], text=True, timeout=30).splitlines()
    cargo = ["cargo"] + (["+" + os.environ["RUST_VERSION"]] if os.environ.get("RUST_VERSION") else [])
    metadata = json.loads(subprocess.check_output(cargo + ["metadata", "--no-deps", "--format-version", "1", "--locked"], text=True, timeout=120))
    packages = [p for p in metadata["packages"] if p["id"] in metadata["workspace_members"]]
    selected = affected(paths, packages, root)
    # Exempt only a proven owned-version-field-only diff. Check from the trusted
    # release, never from a final version-bump commit or PR base alone.
    import importlib.util
    spec = importlib.util.spec_from_file_location("ci_select", root / "scripts/ci-select.py")
    selector = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(selector)
    if selector.version_only(args.base, args.head, paths):
        selected = []
    command = cargo + ["test", "--locked"]
    if args.validation == "full":
        command += ["--workspace"]
    else:
        for name in selected:
            command += ["-p", name]
    print(json.dumps({"validation": args.validation, "packages": selected, "command": command if selected or args.validation == "full" else []}), flush=True)
    if not args.plan_only and (selected or args.validation == "full"):
        subprocess.run(command, check=True, timeout=1800)


if __name__ == "__main__":
    main()
