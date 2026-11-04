#!/usr/bin/env python3
"""Select CI from a complete diff; version fields are parsed conservatively."""
import argparse
import copy
import fnmatch
import json
import re
import subprocess

FLAGS = ("rust", "web", "npm", "workflows", "windows_harness", "thread_contract", "treer_packaging")
VERSION_FILES = {"Cargo.toml", "Cargo.lock", "package.json"}


def git(*args):
    return subprocess.run(["git", *args], check=True, text=True, stdout=subprocess.PIPE, timeout=30).stdout


def normalized_version(path, text):
    if path == "package.json":
        value = json.loads(text)
        version = value.pop("version")
        if not re.fullmatch(r"\d+\.\d+\.\d+", version):
            raise ValueError("Invalid version")
        return value
    section = ""
    owner = ""
    output = []
    count = 0
    for line in text.splitlines(keepends=True):
        if line.startswith("["):
            section = line.strip()
            owner = ""
        name = re.fullmatch(r'name = "([^"]+)"\s*', line)
        if name:
            owner = name[1]
        version = re.fullmatch(r'version = "(\d+\.\d+\.\d+)"\s*', line)
        allowed = (path == "Cargo.toml" and section == "[workspace.package]") or (path == "Cargo.lock" and section == "[[package]]" and (owner == "pockymoe" or owner.startswith("pockymoe-")))
        if version and allowed:
            output.append('version = "<version>"\n')
            count += 1
        else:
            output.append(line)
    if not count:
        raise ValueError("Missing owned version fields")
    return "".join(output)


def version_only(base, head, paths):
    if not paths or not set(paths) <= VERSION_FILES:
        return False
    try:
        return all(normalized_version(p, git("show", f"{base}:{p}")) == normalized_version(p, git("show", f"{head}:{p}")) for p in paths)
    except (ValueError, KeyError, subprocess.CalledProcessError):
        return False


def select(paths, version=False):
    result = dict.fromkeys(FLAGS, False)
    result["version_only"] = version
    if version:
        return result
    patterns = {
        "rust": ["crates/**", "migrations/**", ".cargo/**", "Cargo.*", "rust-toolchain*", "*rustfmt*"],
        "web": ["apps/supervisor-web/**", "themes/**", "packages/**", "package.json", "pnpm*", "tsconfig*", "thread-ui.sha"],
        "npm": ["npm/**", "scripts/*", "package.json"],
        "workflows": [".github/**"],
        "windows_harness": ["crates/runtime/src/acp/rpc.rs", "crates/runtime/src/acp/dependencies.rs", "crates/runtime/src/management.rs"],
        "thread_contract": ["crates/protocol/**", "packages/shared/**", "Cargo.*", ".github/workflows/ci.yml"],
        "treer_packaging": ["integrations/treer/**", ".github/workflows/ci.yml"],
    }
    for path in paths:
        matches = [key for key, globs in patterns.items() if any(fnmatch.fnmatchcase(path, pattern) for pattern in globs)]
        for key in matches:
            result[key] = True
        if not matches and not (path.endswith(".md") or path.startswith(("docs/", ".agents/", "e2e/", "apps/windows-device-manager/")) or path in {"AGENTS.md", "CLAUDE.md", ".gitignore"}):
            # Unknown inputs must not silently select the lightest tier.
            for key in ["rust", "web", "npm"]:
                result[key] = True
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("base")
    parser.add_argument("head")
    parser.add_argument("--github-output")
    args = parser.parse_args()
    base = git("merge-base", args.base, args.head).strip()
    paths = git("diff", "--name-only", "--no-renames", base, args.head).splitlines()
    result = select(paths, version_only(base, args.head, paths))
    if args.github_output:
        with open(args.github_output, "a") as out:
            for key, value in result.items():
                out.write(f"{key}={str(value).lower()}\n")
    print(json.dumps(result, separators=(",", ":")))


if __name__ == "__main__":
    main()
