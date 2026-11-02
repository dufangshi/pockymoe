#!/usr/bin/env python3
"""Audit the live Nginx dump before trusting its client-IP forwarding.

No raw configuration, origin, IP, response body or exception text is printed.
Cloudflare detection is an observation, not proof that no other CDN exists.
"""
import argparse
import fnmatch
import ipaddress
import json
from pathlib import Path
import posixpath
import re
import shlex
import socket
import urllib.request
from urllib.parse import urlsplit


class AuditError(ValueError):
    CODES = {
        'nginx-syntax-unsupported', 'nginx-dump-incomplete', 'nginx-dump-invalid',
        'include-unresolved', 'include-dynamic', 'http-context-not-found',
        'relay-vhost-not-found', 'upstream-unresolved', 'upstream-invalid',
        'upstream-unverified', 'proxy-header-not-forwarded', 'real-ip-policy-unsupported',
        'real-ip-trust-invalid', 'real-ip-trust-incomplete', 'cdn-real-ip-missing',
        'forwarding-conditional', 'relay-upstream-not-found', 'public-health-failed',
        'cdn-policy-unidentified', 'cdn-reference-unavailable', 'origin-invalid',
        'audit-arguments-invalid', 'nginx-dump-unreadable', 'cdn-observation-unavailable',
    }

    def __init__(self, code):
        super().__init__(code if code in self.CODES else 'audit-internal-error')


def parse(text):
    tokens = re.findall(r'"(?:\\.|[^"\\])*"|\x27(?:\\.|[^\x27\\])*\x27|#[^\n]*|[{};]|(?:\$\{[^}]+\}|[^\s{};])+', text)
    tokens = [token for token in tokens if not token.startswith('#')]
    index = 0

    def block(nested=False):
        nonlocal index
        result = []
        words = []
        while index < len(tokens):
            token = tokens[index]
            index += 1
            if token == '}':
                if words or not nested:
                    raise AuditError('nginx-syntax-unsupported')
                return result
            if token in [';', '{']:
                if not words:
                    raise AuditError('nginx-syntax-unsupported')
                result.append((words[0], words[1:], block(True) if token == '{' else []))
                words = []
            else:
                words.append(shlex.split(token)[0] if token[:1] in ['"', "'"] else token)
        if words or nested:
            raise AuditError('nginx-dump-incomplete')
        return result
    return block()


def configuration(dump):
    # NPM/OpenResty can write the test diagnostics to stderr. Capture both
    # streams privately and ignore diagnostic lines, never their raw values.
    dump = re.sub(r'^nginx:.*$', '', dump, flags=re.M)
    parts = re.split(r'^# configuration file ([^\n]+):\n', dump, flags=re.M)
    if len(parts) < 3:
        raise AuditError('nginx-dump-invalid')
    files = {posixpath.normpath(parts[i]): parts[i + 1] for i in range(1, len(parts), 2)}
    root = posixpath.normpath(parts[1])
    prefix = posixpath.dirname(root)

    def load(name, stack=()):
        if name in stack or name not in files:
            raise AuditError('include-unresolved')
        def expand(nodes):
            result = []
            for key, args, children in nodes:
                if key == 'include':
                    if len(args) != 1 or '$' in args[0]:
                        raise AuditError('include-dynamic')
                    pattern = posixpath.normpath(args[0] if args[0].startswith('/') else posixpath.join(prefix, args[0]))
                    matches = sorted(path for path in files if fnmatch.fnmatchcase(path, pattern))
                    if not matches and not any(char in pattern for char in '*?['):
                        raise AuditError('include-unresolved')
                    for match in matches:
                        result.extend(load(match, (*stack, name)))
                else:
                    result.append((key, args, expand(children)))
            return result
        return expand(parse(files[name]))
    return load(root)


def values(nodes, name):
    return [args for key, args, _ in nodes if key == name]


def effective(contexts, name):
    for nodes in reversed(contexts):
        found = values(nodes, name)
        if found:
            return found
    return []


def audit(dump, hostname, cloudflare, networks, client_ip_header="", upstreams=None):
    tree = configuration(dump)
    http = [children for key, _, children in tree if key == 'http']
    if len(http) != 1:
        raise AuditError('http-context-not-found')
    servers = [children for key, _, children in http[0] if key == 'server' and
               any(hostname in args for args in values(children, 'server_name'))]
    if not servers:
        raise AuditError('relay-vhost-not-found')
    checked = 0
    for server in servers:
        contexts = [http[0], server]

        def inspect(nodes, parents):
            nonlocal checked
            contexts = [*parents, nodes]
            variables = {args[0].lstrip('$'): args[1] for context in contexts for args in values(context, 'set') if len(args) == 2}
            for upstream in values(nodes, 'proxy_pass'):
                target = upstream[0]
                def substitute(match):
                    name = match[1] or match[2]
                    return '' if name in ['request_uri', 'uri', 'is_args', 'args'] else variables.get(name, match[0])
                target = re.sub(r'\$\{([A-Za-z_][A-Za-z_0-9]*)\}|\$([A-Za-z_][A-Za-z_0-9]*)', substitute, target)
                if '$' in target:
                    raise AuditError('upstream-unresolved')
                try:
                    parsed = urlsplit(target)
                    host, port = parsed.hostname, parsed.port
                except ValueError:
                    raise AuditError('upstream-invalid')
                # NPM bridge mode legitimately uses a host/gateway alias here.
                # The configuration alone does not prove it reaches this host's
                # Relay: configure-relay-proxy observes that socket before trust.
                if parsed.scheme != 'http' or not host or port != 18791 or parsed.username or parsed.password:
                    raise AuditError('upstream-unverified')
                if upstreams is not None:
                    upstreams.append({'host': host, 'port': port})
                # proxy_set_header inherits as one group, only if this context
                # declares none. A location's Host override can discard parent XFF.
                headers = effective(contexts, 'proxy_set_header')
                if client_ip_header.lower() == 'cf-connecting-ip':
                    explicit = [args[1] for args in headers if len(args)==2 and args[0].lower()=='cf-connecting-ip']
                    passes = effective(contexts, 'proxy_pass_request_headers')
                    if explicit not in [[], ['$http_cf_connecting_ip']] or (not explicit and passes == [['off']]):
                        raise AuditError('proxy-header-not-forwarded')
                    checked += 1
                    continue  # Dedicated visitor header mode does not use XFF/remote_addr.
                forwarded = [args[1] for args in headers if len(args) == 2 and args[0].lower() == 'x-forwarded-for']
                if forwarded not in [['$proxy_add_x_forwarded_for'], ['$remote_addr']]:
                    raise AuditError('proxy-header-not-forwarded')
                header = effective(contexts, 'real_ip_header')
                trusted = effective(contexts, 'set_real_ip_from')
                if header or trusted:
                    if len(header) != 1 or [value.lower() for value in header[0]] != ['cf-connecting-ip'] or not trusted:
                        raise AuditError('real-ip-policy-unsupported')
                    try:
                        trust = [ipaddress.ip_network(args[0]) for args in trusted if len(args) == 1]
                    except ValueError:
                        raise AuditError('real-ip-trust-invalid')
                    if len(trust) != len(trusted) or not networks or any(
                        not any(net.version == official.version and net.subnet_of(official) for official in networks)
                        for net in trust
                    ):
                        raise AuditError('real-ip-trust-invalid')
                    if cloudflare and any(not any(net.version == configured.version and net.subnet_of(configured) for configured in trust) for net in networks):
                        raise AuditError('real-ip-trust-incomplete')
                elif cloudflare:
                    raise AuditError('cdn-real-ip-missing')
                checked += 1
            for key, _, children in nodes:
                if children:
                    if key == 'if' and any(child[0] in ['proxy_pass', 'proxy_set_header', 'set_real_ip_from', 'real_ip_header'] for child in children):
                        raise AuditError('forwarding-conditional')
                    inspect(children, contexts)
        inspect(server, [http[0]])
    if not checked:
        raise AuditError('relay-upstream-not-found')
    return checked


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def probe(origin, networks):
    # Only the non-authenticated health read is sent. The forged prefix is used
    # to probe CDN presence, not claimed as visibility into Relay's resolved IP.
    request = urllib.request.Request(origin + '/healthz', headers={
        'User-Agent': 'Pockymoe-ingress-preflight', 'X-Forwarded-For': '203.0.113.234',
        'CF-Connecting-IP': '203.0.113.235', 'Cache-Control': 'no-cache'})
    with urllib.request.build_opener(NoRedirect()).open(request, timeout=20) as response:
        if response.status != 200 or b'"status"' not in response.read(65536):
            raise AuditError('public-health-failed')
        via = response.headers.get('via', '')
        cf = bool(response.headers.get('cf-ray')) or 'cloudflare' in response.headers.get('server', '').lower()
        if via and not cf:
            raise AuditError('cdn-policy-unidentified')
    addresses = [ipaddress.ip_address(row[4][0]) for row in socket.getaddrinfo(urlsplit(origin).hostname, 443, type=socket.SOCK_STREAM)]
    return cf or any(any(ip.version == net.version and ip in net for net in networks) for ip in addresses)


def official_networks():
    result = []
    for family in [4, 6]:
        with urllib.request.urlopen(f'https://www.cloudflare.com/ips-v{family}', timeout=20) as response:
            result.extend(ipaddress.ip_network(line) for line in response.read(65536).decode().splitlines() if line.strip())
    if not result or not any(net.version == 4 for net in result) or not any(net.version == 6 for net in result):
        raise AuditError('cdn-reference-unavailable')
    return result


class SafeParser(argparse.ArgumentParser):
    def error(self, message):
        raise AuditError('audit-arguments-invalid')


def main(argv=None):
    parser = SafeParser(description=__doc__)
    parser.add_argument('--nginx-dump', type=Path, required=True)
    parser.add_argument('--origin', required=True)
    parser.add_argument('--client-ip-header', choices=['CF-Connecting-IP'], default='')
    parser.add_argument('--upstreams-out', type=Path)
    try:
        args = parser.parse_args(argv)
        parsed = urlsplit(args.origin)
        if parsed.scheme != 'https' or not parsed.hostname or parsed.path or parsed.username or parsed.password or parsed.query or parsed.fragment:
            raise AuditError('origin-invalid')
        try:
            dump = args.nginx_dump.read_text()
        except (OSError, UnicodeError):
            raise AuditError('nginx-dump-unreadable')
        # Explicit CDN header mode does not need a public health/DNS call to
        # establish the operator-confirmed topology. Such a call cannot prove
        # origin isolation and must not indefinitely block a sound local audit.
        networks, cf = [], False
        if not args.client_ip_header:
            try:
                networks = official_networks()
                cf = probe(args.origin, networks)
            except Exception:
                raise AuditError('cdn-observation-unavailable')
        upstreams = []
        count = audit(dump, parsed.hostname, cf, networks, args.client_ip_header, upstreams)
        if args.upstreams_out:
            import os
            fd = os.open(args.upstreams_out, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, 'w') as output:
                os.fchmod(output.fileno(), 0o600)
                json.dump(upstreams, output)
    except AuditError as error:
        print(f'FAIL: {error}')
        return 1
    except (ValueError, IndexError):
        print('FAIL: nginx-syntax-unsupported')
        return 1
    except Exception:
        print('FAIL: audit-internal-error')
        return 1
    print(f'PASS: forwarding-config-audited ({count} contexts)')
    if args.client_ip_header:
        print('WARN: origin-isolation-unverified')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
