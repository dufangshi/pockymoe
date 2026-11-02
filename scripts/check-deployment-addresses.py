#!/usr/bin/env python3
"""Fail before deployment if deployment address secrets are absent/malformed."""
import argparse
import os
import re
from urllib.parse import urlsplit


def validate(env):
    for name in ['POCKYMOE_RELAY_ORIGIN', 'POCKYMOE_LEGACY_RELAY_ORIGIN']:
        value = env.get(name, '')
        url = urlsplit(value)
        if (url.scheme != 'https' or not url.hostname or url.username or url.password
                or url.path or url.query or url.fragment or url.port or value.endswith('/')):
            raise ValueError(f'{name} must be an HTTPS origin without a trailing slash')
    domain = env.get('POCKYMOE_WEB_DOMAIN', '')
    if len(domain) > 253 or '.' not in domain or not all(
        re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?', label)
        for label in domain.split('.')
    ):
        raise ValueError('POCKYMOE_WEB_DOMAIN must be a lowercase DNS hostname')


def mask_values(env):
    values = set()
    for name in ['POCKYMOE_RELAY_ORIGIN', 'POCKYMOE_LEGACY_RELAY_ORIGIN', 'POCKYMOE_WEB_DOMAIN']:
        value = env.get(name, '')
        if value:
            values.add(value)
            try:
                host = urlsplit(value).hostname if '://' in value else value
                if host:
                    values.update([host, f'https://{host}', f'wss://{host}'])
            except ValueError:
                pass  # Mask the raw secret even when validation will reject it.
    return sorted(values)


def mask(env):
    for value in mask_values(env):
        # Escape workflow-command data, including malformed multiline inputs.
        escaped = value.replace('%', '%25').replace('\r', '%0D').replace('\n', '%0A')
        print(f'::add-mask::{escaped}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mask', action='store_true')
    args = parser.parse_args()
    if args.mask:
        mask(os.environ)
    try:
        validate(os.environ)
    except ValueError:
        # URL parser exceptions can include malformed secret components.
        raise SystemExit('Configure valid POCKYMOE_RELAY_ORIGIN, POCKYMOE_WEB_DOMAIN and POCKYMOE_LEGACY_RELAY_ORIGIN deployment secrets')
    print('PASS: deployment address secrets are configured')
