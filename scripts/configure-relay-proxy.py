#!/usr/bin/env python3
"""Observe Docker's actual Relay peer, then privately back up and configure trust.

All public output consists of fixed diagnostic codes. Container metadata,
upstream values and kernel socket addresses remain private to this process.
"""
import argparse
import ipaddress
import json
import os
from pathlib import Path
import re
import selectors
import socket
import stat
import subprocess
import time


class ConfigError(ValueError):
    CODES = {
        'proxy-container-not-running', 'docker-network-unsupported', 'docker-network-unresolved',
        'docker-inspect-failed', 'socket-table-unavailable', 'proxy-connect-timeout',
        'proxy-connect-failed', 'peer-not-observed', 'peer-not-container', 'relay-upstream-not-found',
        'upstream-unverified', 'peer-invalid', 'environment-symlink', 'environment-backup-not-private',
        'peer-trust-invalid', 'peer-not-trusted', 'visitor-header-config-invalid',
        'configure-arguments-invalid', 'peer-evidence-unwritable',
    }

    def __init__(self, code):
        super().__init__(code if code in self.CODES else 'environment-configure-failed')


def normalized_ip(value):
    ip = ipaddress.ip_address(value)
    return ip.ipv4_mapped if isinstance(ip, ipaddress.IPv6Address) and ip.ipv4_mapped else ip


def container_metadata(container):
    try:
        selection = '{"State":{"Running":{{json .State.Running}}},"HostConfig":{"NetworkMode":{{json .HostConfig.NetworkMode}}},"NetworkSettings":{"Networks":{{json .NetworkSettings.Networks}}}}'
        result = subprocess.run(['docker', 'inspect', '--format', selection, container], capture_output=True, text=True, timeout=15, check=True)
        data = json.loads(result.stdout)
        return metadata_peers(data)
    except ConfigError:
        raise
    except Exception:
        raise ConfigError('docker-inspect-failed')


def metadata_peers(data):
    try:
        mode = data['HostConfig']['NetworkMode']
        if not data['State']['Running']:
            raise ConfigError('proxy-container-not-running')
        peers = set()
        if mode == 'host':
            return mode, peers
        if mode == 'none' or mode.startswith('container:'):
            raise ConfigError('docker-network-unsupported')
        for network in data['NetworkSettings']['Networks'].values():
            for field in ['IPAddress', 'GlobalIPv6Address', 'Gateway', 'IPv6Gateway']:
                if network.get(field):
                    peers.add(normalized_ip(network[field]))
        if not peers:
            raise ConfigError('docker-network-unresolved')
        return mode, peers
    except ConfigError:
        raise
    except Exception:
        raise ConfigError('docker-inspect-failed')


def observed_peers(client_port, relay_port, proc=Path('/proc/net')):
    peers = set()
    for name, family in [('tcp', socket.AF_INET), ('tcp6', socket.AF_INET6)]:
        try:
            lines = (proc / name).read_text().splitlines()[1:]
        except FileNotFoundError:
            if name == 'tcp6':
                continue
            raise ConfigError('socket-table-unavailable')
        except OSError:
            raise ConfigError('socket-table-unavailable')
        for line in lines:
            columns = line.split()
            local, remote = columns[1].split(':'), columns[2].split(':')
            if columns[3] != '01' or int(local[1], 16) != relay_port or int(remote[1], 16) != client_port:
                continue
            raw = bytes.fromhex(remote[0])
            # Linux proc uses native-endian words for both address families.
            raw = b''.join(raw[i:i + 4][::-1] for i in range(0, len(raw), 4))
            ip = ipaddress.ip_address(socket.inet_ntop(family, raw))
            peers.add(ip.ipv4_mapped if isinstance(ip, ipaddress.IPv6Address) and ip.ipv4_mapped else ip)
    return peers


def observe_peer(container, upstream, mode, expected):
    # A TCP connect only: no HTTP request, authentication, body or native token.
    # NPM includes Node; the client self-exits even if docker exec is interrupted.
    code = "const net=require('net');const c=net.createConnection({host:process.argv[1],port:Number(process.argv[2])});c.on('connect',()=>console.log(JSON.stringify({port:c.localPort,source:c.localAddress})));c.on('error',()=>process.exit(1));setTimeout(()=>c.destroy(),10000).unref();"
    child = None
    try:
        child = subprocess.Popen(['docker', 'exec', container, 'node', '-e', code, upstream['host'], str(upstream['port'])], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        with selectors.DefaultSelector() as ready:
            ready.register(child.stdout, selectors.EVENT_READ)
            if not ready.select(10):
                raise ConfigError('proxy-connect-timeout')
            connected = json.loads(child.stdout.readline())
            port = connected['port']
            source = ipaddress.ip_address(connected['source'])
            if isinstance(source, ipaddress.IPv6Address) and source.ipv4_mapped:
                source = source.ipv4_mapped
        if not isinstance(port, int) or not 0 < port < 65536:
            raise ConfigError('proxy-connect-failed')
        deadline = time.monotonic() + 2
        peers = set()
        while time.monotonic() < deadline:
            peers = observed_peers(port, upstream['port'])
            if peers:
                break
            time.sleep(0.05)
        if len(peers) != 1:
            raise ConfigError('peer-not-observed')
        peer = next(iter(peers))
        # A Docker gateway is a destination, not necessarily the TCP source.
        # Trust exactly the observed kernel peer, constrained to this container's
        # host/bridge metadata. Never infer trust for the whole bridge subnet.
        if (mode == 'host' and peer != source) or (mode != 'host' and peer not in expected):
            raise ConfigError('peer-not-container')
        return str(peer)
    except ConfigError:
        raise
    except Exception:
        raise ConfigError('proxy-connect-failed')
    finally:
        if child:
            child.terminate()
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()


def derive_peers(container, upstreams):
    mode, expected = container_metadata(container)
    if not upstreams:
        raise ConfigError('relay-upstream-not-found')
    result = set()
    for upstream in {json.dumps(item, sort_keys=True) for item in upstreams}:
        item = json.loads(upstream)
        if not isinstance(item.get('host'), str) or item.get('port') != 18791:
            raise ConfigError('upstream-unverified')
        result.add(observe_peer(container, item, mode, expected))
    return sorted(result)


def configure(path, backup, peers):
    try:
        peers = sorted({str(normalized_ip(peer)) for peer in peers})
    except ValueError:
        raise ConfigError('peer-invalid')
    if not peers:
        raise ConfigError('peer-not-observed')
    if path.is_symlink() or backup.is_symlink():
        raise ConfigError('environment-symlink')
    original = path.read_bytes()
    backup.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(backup.parent, 0o700)
    try:
        fd = os.open(backup, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        if stat.S_IMODE(backup.stat().st_mode) & 0o077:
            raise ConfigError('environment-backup-not-private')
    else:
        with os.fdopen(fd, 'wb') as out:
            out.write(original)
            out.flush()
            os.fsync(out.fileno())
    values = {}
    for line in original.decode().splitlines():
        match = re.match(r'^\s*([A-Za-z_][A-Za-z_0-9]*)\s*=(.*)$', line)
        if match:
            import shlex
            parts = shlex.split(match[2], comments=True)
            values[match[1]] = parts[0] if parts else ''
    configured = values.get('POCKYMOE_TRUSTED_PROXY_IPS', values.get('REMOTE_CODEX_TRUSTED_PROXY_IPS'))
    if configured is not None:
        try:
            existing = {str(normalized_ip(item.strip())) for item in configured.split(',') if item.strip()}
        except ValueError:
            raise ConfigError('peer-trust-invalid')
        if not set(peers) <= existing:
            raise ConfigError('peer-not-trusted')
    header = values.get('POCKYMOE_CLIENT_IP_HEADER', values.get('REMOTE_CODEX_CLIENT_IP_HEADER'))
    if header is not None and header.strip().lower() != 'cf-connecting-ip':
        raise ConfigError('visitor-header-config-invalid')
    defaults = [('POCKYMOE_TRUSTED_PROXY_IPS', 'REMOTE_CODEX_TRUSTED_PROXY_IPS', ','.join(peers)),
                ('POCKYMOE_CLIENT_IP_HEADER', 'REMOTE_CODEX_CLIENT_IP_HEADER', 'CF-Connecting-IP')]
    missing = [(key, value) for key, legacy, value in defaults if key not in values and legacy not in values]
    if not missing:
        return False
    addition = (b'' if original.endswith(b'\n') else b'\n') + ''.join(f'{key}={value}\n' for key, value in missing).encode()
    with path.open('ab') as out:
        out.write(addition)
        out.flush()
        os.fsync(out.fileno())
    return True


def main(argv=None):
    class SafeParser(argparse.ArgumentParser):
        def error(self, message):
            raise ConfigError('configure-arguments-invalid')
    parser = SafeParser(description=__doc__)
    parser.add_argument('--env', type=Path, required=True)
    parser.add_argument('--backup', type=Path, required=True)
    parser.add_argument('--container', required=True)
    parser.add_argument('--upstreams', type=Path, required=True)
    parser.add_argument('--observed-peers-out', type=Path)
    try:
        args = parser.parse_args(argv)
        peers = derive_peers(args.container, json.loads(args.upstreams.read_text()))
        if args.observed_peers_out:
            try:
                fd = os.open(args.observed_peers_out, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
                with os.fdopen(fd, 'w') as evidence:
                    os.fchmod(evidence.fileno(), 0o600)
                    json.dump(peers, evidence)
            except OSError:
                raise ConfigError('peer-evidence-unwritable')
        changed = configure(args.env, args.backup, peers)
    except ConfigError as error:
        print(f'FAIL: {error}')
        return 1
    except Exception:
        print('FAIL: environment-configure-failed')
        return 1
    print('PASS: proxy-peer-observed')
    print('PASS: missing-proxy-defaults-added' if changed else 'PASS: existing-proxy-defaults-preserved')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
