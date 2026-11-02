#!/usr/bin/env python3
"""Back up relay.env privately and append missing deployment proxy/header defaults."""
import argparse
import os
from pathlib import Path
import re
import stat

DEFAULTS = [
    ('POCKYMOE_TRUSTED_PROXY_IPS', 'REMOTE_CODEX_TRUSTED_PROXY_IPS', '127.0.0.1'),
    ('POCKYMOE_CLIENT_IP_HEADER', 'REMOTE_CODEX_CLIENT_IP_HEADER', 'CF-Connecting-IP'),
]


def configure(path, backup):
    if path.is_symlink() or backup.is_symlink():
        raise ValueError('Refuse a symlink environment or backup')
    original = path.read_bytes()
    # This backup is the pre-change version and must never be overwritten by a
    # rerun or the later database backup in the deployment workflow.
    backup.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(backup.parent, 0o700)
    try:
        fd = os.open(backup, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        if stat.S_IMODE(backup.stat().st_mode) & 0o077:
            raise ValueError('Existing environment backup is not private')
    else:
        with os.fdopen(fd, 'wb') as out:
            out.write(original)
            out.flush()
            os.fsync(out.fileno())
    keys = set()
    for line in original.decode().splitlines():
        match = re.match(r'^\s*([A-Za-z_][A-Za-z_0-9]*)\s*=', line)
        if match:
            keys.add(match[1])
    missing = [(key, value) for key, legacy, value in DEFAULTS if key not in keys and legacy not in keys]
    if not missing:
        return False  # Preserve explicit empty/invalid values; preflight validates.
    addition = (b'' if original.endswith(b'\n') else b'\n') + ''.join(f'{key}={value}\n' for key, value in missing).encode()
    # Open the existing inode to retain its owner, mode and service-manager path.
    with path.open('ab') as out:
        out.write(addition)
        out.flush()
        os.fsync(out.fileno())
    return True


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--env', type=Path, required=True)
    parser.add_argument('--backup', type=Path, required=True)
    args = parser.parse_args()
    try:
        changed = configure(args.env, args.backup)
    except (OSError, UnicodeError, ValueError):
        raise SystemExit('Cannot safely back up/configure Relay proxy trust; environment details withheld')
    print('Relay proxy trust default added after private backup' if changed else 'Existing Relay proxy trust preserved; private backup retained')
