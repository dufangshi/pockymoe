#!/usr/bin/env python3
"""Fail before deployment if repository address variables are absent/malformed."""
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


if __name__ == '__main__':
    try:
        validate(os.environ)
    except ValueError as error:
        raise SystemExit(str(error))
    print('PASS: deployment address variables are configured')
