#!/usr/bin/env python3
"""Install one explicit GitHub runtime release, without starting any service."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import urllib.request
import uuid
import zipfile

REPO = "https://github.com/dufangshi/pockymoe"
LIMIT = 256 * 1024 * 1024


def version(value):
    value = value.removeprefix("v")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", value):
        raise ValueError("Specify a pinned stable release, for example --version 0.13.1")
    return value


def asset_name():
    key = (sys.platform, platform.machine().lower())
    names = {("linux", "x86_64"): "remote-codex-linux-x64-gnu",
             ("linux", "aarch64"): "remote-codex-linux-arm64-gnu",
             ("darwin", "arm64"): "remote-codex-darwin-arm64"}
    if key not in names:
        raise ValueError("This Treer packaging supports Linux x64/ARM64 and Apple Silicon")
    return names[key]


class HttpsOnly(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not newurl.startswith("https://"):
            raise ValueError("Release redirect must use HTTPS")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def download(url, target, limit=LIMIT):
    request = urllib.request.Request(url, headers={"User-Agent": "pockymoe-treer-install"})
    opener = urllib.request.build_opener(HttpsOnly())
    with opener.open(request, timeout=60) as response, target.open("xb") as output:
        total = 0
        while chunk := response.read(64 * 1024):
            total += len(chunk)
            if total > limit:
                raise ValueError("Release asset exceeds size limit")
            output.write(chunk)
        output.flush()
        os.fsync(output.fileno())


def checksums(text):
    result = {}
    for line in text.splitlines():
        if not line.strip():
            continue
        parts = line.split()
        if len(parts) != 2 or not re.fullmatch(r"[0-9a-fA-F]{64}", parts[0]):
            raise ValueError("Invalid release checksum entry")
        name = parts[1].removeprefix("*")
        if name in result or name in ("", ".", "..") or "/" in name or "\\" in name:
            raise ValueError("Ambiguous release checksum entry")
        result[name] = parts[0].lower()
    return result


def digest(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def unpack_web(archive, target):
    target.mkdir(mode=0o700)
    with zipfile.ZipFile(archive) as bundle:
        entries = bundle.infolist()
        if len(entries) > 5000 or sum(e.file_size for e in entries) > LIMIT:
            raise ValueError("Web archive exceeds size limit")
        seen = set()
        for entry in entries:
            relative = PurePosixPath(entry.filename)
            mode = entry.external_attr >> 16
            if (relative.is_absolute() or ".." in relative.parts or "\\" in entry.filename
                    or ":" in entry.filename or not relative.parts
                    or stat.S_ISLNK(mode) or relative in seen):
                raise ValueError("Unsafe Web archive entry")
            seen.add(relative)
            destination = target.joinpath(*relative.parts)
            if entry.is_dir():
                destination.mkdir(parents=True, exist_ok=True, mode=0o700)
            else:
                destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                with bundle.open(entry) as source, destination.open("xb") as output:
                    shutil.copyfileobj(source, output)
    if not (target / "index.html").is_file():
        raise ValueError("Release Web bundle has no index.html")


def atomic_json(path, value):
    stage = path.with_name(f".{path.name}-{uuid.uuid4()}")
    with stage.open("x") as output:
        json.dump(value, output, indent=2)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    os.replace(stage, path)


def install(selected, home):
    selected = version(selected)
    asset = asset_name()
    native = home / ".local/share/remote-codex/native"
    native.mkdir(parents=True, exist_ok=True, mode=0o700)
    lock = native / ".treer-install.lock"
    lock.mkdir(mode=0o700)  # Never guess whether an existing lock can be broken.
    try:
        # Existing devices update through Settings, not a second installer.
        if os.path.lexists(native / "current") or os.path.lexists(native / "current.json"):
            raise ValueError("An installation is already active; use Supervisor Settings Check/Update")
        releases = native / "releases"
        releases.mkdir(exist_ok=True, mode=0o700)
        target = releases / selected
        if os.path.lexists(target):
            raise ValueError("Pinned release directory already exists; inspect it before retrying")
        bin_dir = home / ".local/bin"
        bin_dir.mkdir(parents=True, exist_ok=True)
        commands = [bin_dir / name for name in ("pockymoe", "remote-codex")]
        if any(os.path.lexists(path) for path in commands):
            raise ValueError("Local CLI paths already exist; refusing to replace them")
        with tempfile.TemporaryDirectory(prefix="stage-treer-", dir=releases) as temporary:
            stage = Path(temporary)
            base = f"{REPO}/releases/download/v{selected}"
            sums_file = stage / "SHA256SUMS"
            download(f"{base}/SHA256SUMS", sums_file, 1024 * 1024)
            sums = checksums(sums_file.read_text())
            binary = stage / "remote-codex"
            web = stage / "remote-codex-web.zip"
            for name, destination in ((asset, binary), (web.name, web)):
                if name not in sums:
                    raise ValueError("Release is missing a required checksum")
                download(f"{base}/{name}", destination)
                if digest(destination) != sums[name]:
                    raise ValueError("Release checksum mismatch")
            binary.chmod(0o700)
            checked = subprocess.run([str(binary), "version"], capture_output=True, timeout=10, check=False)
            if checked.returncode or checked.stdout.decode().strip() != selected:
                raise ValueError("Downloaded runtime version mismatch")
            unpack_web(web, stage / "web")
            web.unlink()
            sums_file.unlink()
            installed = {"version": selected, "executable": str(target / "remote-codex"),
                         "webDist": str(target / "web")}
            atomic_json(stage / "installed.json", installed)
            os.rename(stage, target)
        # Match the native updater's layout and pointers. Never save releases/V in a profile.
        (native / "current").symlink_to(target, target_is_directory=True)
        atomic_json(native / "current.json", installed)
        for command in commands:
            command.symlink_to(native / "current/remote-codex")
        return bin_dir / "pockymoe"
    finally:
        lock.rmdir()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="Exact immutable stable GitHub runtime release; never latest")
    args = parser.parse_args()
    os.umask(0o077)
    try:
        path = install(args.version, Path.home())
    except (OSError, ValueError, subprocess.SubprocessError, UnicodeError, zipfile.BadZipFile):
        # Do not forward network/subprocess diagnostics or environment contents.
        print("Pinned Pockymoe installation failed; check the version, existing layout, lock and release checksums.", file=sys.stderr)
        return 1
    print(f"Installed pinned Pockymoe {version(args.version)} at {path}; no service started.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
