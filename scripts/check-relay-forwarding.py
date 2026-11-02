#!/usr/bin/env python3
"""Audit the live Nginx dump before trusting its client-IP forwarding.

No raw configuration, origin, IP, response body or exception text is printed.
Cloudflare detection is an observation, not proof that no other CDN exists.
"""
import argparse
import fnmatch
import ipaddress
from pathlib import Path
import posixpath
import re
import shlex
import socket
import urllib.request
from urllib.parse import urlsplit


class AuditError(ValueError):
    pass


def parse(text):
    tokens = re.findall(r'"(?:\\.|[^"\\])*"|\x27(?:\\.|[^\x27\\])*\x27|#[^\n]*|[{};]|[^\s{};]+', text)
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
                    raise AuditError('Unsupported Nginx syntax')
                return result
            if token in [';', '{']:
                if not words:
                    raise AuditError('Unsupported Nginx directive')
                result.append((words[0], words[1:], block(True) if token == '{' else []))
                words = []
            else:
                words.append(shlex.split(token)[0] if token[:1] in ['"', "'"] else token)
        if words or nested:
            raise AuditError('Incomplete Nginx dump')
        return result
    return block()


def configuration(dump):
    parts = re.split(r'^# configuration file ([^\n]+):\n', dump, flags=re.M)
    if len(parts) < 3:
        raise AuditError('Expected nginx -T output with loaded file markers')
    files = {posixpath.normpath(parts[i]): parts[i + 1] for i in range(1, len(parts), 2)}
    root = posixpath.normpath(parts[1])
    prefix = posixpath.dirname(root)

    def load(name, stack=()):
        if name in stack or name not in files:
            raise AuditError('Unresolved or recursive Nginx include')
        def expand(nodes):
            result = []
            for key, args, children in nodes:
                if key == 'include':
                    if len(args) != 1 or '$' in args[0]:
                        raise AuditError('Dynamic Nginx include cannot be verified')
                    pattern = posixpath.normpath(args[0] if args[0].startswith('/') else posixpath.join(prefix, args[0]))
                    matches = sorted(path for path in files if fnmatch.fnmatchcase(path, pattern))
                    if not matches and not any(char in pattern for char in '*?['):
                        raise AuditError('Loaded Nginx include missing from dump')
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


def audit(dump, hostname, cloudflare, networks):
    tree = configuration(dump)
    http = [children for key, _, children in tree if key == 'http']
    if len(http) != 1:
        raise AuditError('Cannot identify Nginx http context')
    servers = [children for key, _, children in http[0] if key == 'server' and
               any(hostname in args for args in values(children, 'server_name'))]
    if not servers:
        raise AuditError('Cannot identify exact Relay virtual host; review ingress before deploy')
    checked = 0
    for server in servers:
        contexts = [http[0], server]
        variables = {args[0]: args[1] for nodes in contexts for args in values(nodes, 'set') if len(args) == 2}

        def inspect(nodes, parents):
            nonlocal checked
            contexts = [*parents, nodes]
            for upstream in values(nodes, 'proxy_pass'):
                target = upstream[0]
                for name, value in variables.items():
                    target = target.replace(name, value)
                target = target.replace('$request_uri', '')
                parsed = urlsplit(target)
                if parsed.hostname not in ['127.0.0.1', 'localhost'] or parsed.port != 18791:
                    raise AuditError('Relay virtual host has an unverified upstream')
                # proxy_set_header inherits as one group, only if this context
                # declares none. A location's Host override can discard parent XFF.
                headers = effective(contexts, 'proxy_set_header')
                forwarded = [args[1] for args in headers if len(args) == 2 and args[0].lower() == 'x-forwarded-for']
                if forwarded not in [['$proxy_add_x_forwarded_for'], ['$remote_addr']]:
                    raise AuditError('Relay ingress must append/overwrite X-Forwarded-For using remote_addr')
                header = effective(contexts, 'real_ip_header')
                trusted = effective(contexts, 'set_real_ip_from')
                if header or trusted:
                    if len(header) != 1 or [value.lower() for value in header[0]] != ['cf-connecting-ip'] or not trusted:
                        raise AuditError('Real-IP policy is not the audited Cloudflare normalization policy')
                    try:
                        trust = [ipaddress.ip_network(args[0]) for args in trusted if len(args) == 1]
                    except ValueError:
                        raise AuditError('Unverifiable real-IP trust range')
                    if len(trust) != len(trusted) or not networks or any(
                        not any(net.version == official.version and net.subnet_of(official) for official in networks)
                        for net in trust
                    ):
                        raise AuditError('Real-IP must trust only current official Cloudflare ranges')
                    if cloudflare and any(not any(net.version == configured.version and net.subnet_of(configured) for configured in trust) for net in networks):
                        raise AuditError('Cloudflare detected but real-IP trust does not cover all current edge ranges')
                elif cloudflare:
                    raise AuditError('Cloudflare detected without CF-Connecting-IP normalization; shared-CDN lockout would remain')
                checked += 1
            for key, _, children in nodes:
                if children:
                    if key == 'if' and any(child[0] in ['proxy_pass', 'proxy_set_header', 'set_real_ip_from', 'real_ip_header'] for child in children):
                        raise AuditError('Conditional forwarding cannot be verified automatically')
                    inspect(children, contexts)
        inspect(server, [http[0]])
    if not checked:
        raise AuditError('Relay upstream forwarding not found in active configuration')
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
            raise AuditError('Public Relay health probe failed')
        via = response.headers.get('via', '')
        cf = bool(response.headers.get('cf-ray')) or 'cloudflare' in response.headers.get('server', '').lower()
        if via and not cf:
            raise AuditError('Unidentified intermediary detected; review its client-IP policy before deployment')
    addresses = [ipaddress.ip_address(row[4][0]) for row in socket.getaddrinfo(urlsplit(origin).hostname, 443, type=socket.SOCK_STREAM)]
    return cf or any(any(ip.version == net.version and ip in net for net in networks) for ip in addresses)


def official_networks():
    result = []
    for family in [4, 6]:
        with urllib.request.urlopen(f'https://www.cloudflare.com/ips-v{family}', timeout=20) as response:
            result.extend(ipaddress.ip_network(line) for line in response.read(65536).decode().splitlines() if line.strip())
    if not result or not any(net.version == 4 for net in result) or not any(net.version == 6 for net in result):
        raise AuditError('Cloudflare trust reference unavailable')
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--nginx-dump', type=Path, required=True)
    parser.add_argument('--origin', required=True)
    args = parser.parse_args()
    try:
        parsed = urlsplit(args.origin)
        if parsed.scheme != 'https' or not parsed.hostname or parsed.path or parsed.username or parsed.password or parsed.query or parsed.fragment:
            raise AuditError('Invalid Relay origin')
        networks = official_networks()
        cf = probe(args.origin, networks)
        count = audit(args.nginx_dump.read_text(), parsed.hostname, cf, networks)
    except AuditError as error:
        raise SystemExit(str(error))
    except Exception:
        raise SystemExit('Ingress preflight failed; network/configuration details withheld')
    print(f'PASS: {count} Relay upstream contexts append/overwrite the resolved remote address')
    print('Cloudflare observed; verified edge-only CF-Connecting-IP normalization' if cf else
          'No Cloudflare observed by health headers/DNS; other hidden CDNs are not excluded')
    print('Configuration audit only: health responses do not disclose the Relay-resolved IP; no end-to-end attribution claim')
