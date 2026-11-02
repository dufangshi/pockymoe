#!/usr/bin/env python3
"""Reject the production address without publishing its spelling in this guard.

Tracked source has one frozen exception: the independently released Windows
Device Manager manifest. Artifacts have no exceptions. ZIP members are scanned
uncompressed, as are UTF-16 executable strings and escaped dotted hostnames.
"""
import argparse
import hashlib
import io
from pathlib import Path
import re
import subprocess
import zipfile

FORBIDDEN_HASHES = {'5c353ee70c5c6078f0bb6d722564afdd030cdf6f68f7563dc126e2e035b21b23': 13}
# Only these exact pre-existing bytes are exempt; changing the manifest requires
# resolving its independent release contract instead of expanding an allowlist.
FROZEN_MANIFEST = 'apps/windows-device-manager/ProductManifest.cs'
FROZEN_MANIFEST_HASH = 'a9fa5b3676dc2b4cdecbc192d68b5c0558b865bc60b34d7b8f78271d28bb50eb'
HOSTNAME = re.compile(rb'(?<![a-z0-9-])[a-z0-9-]+(?:\.[a-z0-9-]+)+', re.I)


def contains_address(data, forbidden=FORBIDDEN_HASHES):
    variants = [data]
    if b'\x00' in data:
        variants.append(data.replace(b'\x00', b''))
    for variant in variants:
        variant = variant.lower().replace(b'\\.', b'.')
        # JSON/JS may spell punctuation or letters with Unicode escapes.
        variant = re.sub(rb'\\u00([0-9a-f]{2})', lambda m: bytes([int(m[1], 16)]), variant).lower()
        for match in HOSTNAME.finditer(variant):
            candidate = match[0]
            # Fixed-size windows are linear even in minified dotted identifiers;
            # checking every spelling window also catches adjacent binary text.
            for length in set(forbidden.values()):
                for offset in range(len(candidate) - length + 1):
                    if hashlib.sha256(candidate[offset:offset + length]).hexdigest() in forbidden:
                        return True
    return False


def scan_blob(data, label, forbidden=FORBIDDEN_HASHES):
    if contains_address(data, forbidden):
        raise ValueError(f'Production address found in {label}')
    if zipfile.is_zipfile(io.BytesIO(data)):
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            for index, member in enumerate(archive.infolist(), 1):
                # Do not echo an untrusted member name: it could contain the address.
                scan_blob(member.filename.encode(), f'{label}: ZIP member name', forbidden)
                if not member.is_dir():
                    scan_blob(archive.read(member), f'{label}: ZIP member #{index}', forbidden)


def scan_tracked():
    paths = subprocess.check_output(['git', 'ls-files', '-z']).decode().split('\0')
    count = 0
    for name in filter(None, paths):
        path = Path(name)
        if not path.is_file():
            continue  # Separate repository gitlinks are checked in their own repo.
        data = path.read_bytes()
        if name == FROZEN_MANIFEST and hashlib.sha256(data).hexdigest() == FROZEN_MANIFEST_HASH:
            continue
        scan_blob(data, f'tracked file #{count + 1}')
        count += 1
    print(f'PASS: {count} tracked files; one frozen independent-product exception')


def scan_paths(paths):
    count = 0
    for path in paths:
        for file in sorted(path.rglob('*')) if path.is_dir() else [path]:
            if file.is_file():
                scan_blob(file.name.encode(), f'artifact #{count + 1} filename')
                scan_blob(file.read_bytes(), f'artifact #{count + 1}')
                count += 1
    if count == 0:
        raise ValueError('No artifact files to scan')
    print(f'PASS: {count} artifact files (ZIP members decoded); no exceptions')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tracked', action='store_true')
    parser.add_argument('paths', type=Path, nargs='*')
    args = parser.parse_args()
    try:
        if not args.tracked and not args.paths:
            parser.error('Select --tracked or artifact paths')
        if args.tracked:
            scan_tracked()
        if args.paths:
            scan_paths(args.paths)
    except (ValueError, OSError, zipfile.BadZipFile) as error:
        raise SystemExit(str(error))
