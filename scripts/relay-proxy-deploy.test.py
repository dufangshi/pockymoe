#!/usr/bin/env python3
import importlib.util
import contextlib
import io
import json
import socket
from types import SimpleNamespace
import ipaddress
from pathlib import Path
import stat
import tempfile
import unittest
from unittest.mock import patch


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), Path(__file__).with_name(name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


configure = load('configure-relay-proxy')
configure_env = lambda env, backup: configure.configure(env, backup, ['127.0.0.1'])
audit = load('check-relay-forwarding')
NETWORKS = [ipaddress.ip_network('192.0.2.0/24'), ipaddress.ip_network('2001:db8::/32')]


def dump(server, http=''):
    return '# configuration file /etc/nginx/nginx.conf:\nhttp { ' + http + ' server { server_name relay.example.com; ' + server + ' } }\n'


class ConfigureProxyTests(unittest.TestCase):
    def test_backup_precedes_idempotent_append_and_retains_permissions(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            env, backup = root / 'relay.env', root / 'backup/relay.env'
            original = b'HOST=127.0.0.1\nPRIVATE_VALUE=synthetic'
            env.write_bytes(original)
            env.chmod(0o640)
            self.assertTrue(configure_env(env, backup))
            expected = original + b'\nPOCKYMOE_TRUSTED_PROXY_IPS=127.0.0.1\nPOCKYMOE_CLIENT_IP_HEADER=CF-Connecting-IP\n'
            self.assertEqual(env.read_bytes(), expected)
            self.assertEqual(backup.read_bytes(), original)
            self.assertEqual(stat.S_IMODE(backup.stat().st_mode), 0o600)
            self.assertEqual(stat.S_IMODE(backup.parent.stat().st_mode), 0o700)
            self.assertEqual(stat.S_IMODE(env.stat().st_mode), 0o640)
            self.assertFalse(configure_env(env, backup))
            self.assertEqual(env.read_bytes(), expected)
            self.assertEqual(backup.read_bytes(), original)

    def test_preserves_each_existing_canonical_legacy_or_empty_setting_independently(self):
        for proxy in ['POCKYMOE_TRUSTED_PROXY_IPS=127.0.0.1,10.0.0.1', 'POCKYMOE_TRUSTED_PROXY_IPS=', 'REMOTE_CODEX_TRUSTED_PROXY_IPS=127.0.0.1']:
            for header in ['POCKYMOE_CLIENT_IP_HEADER=CF-Connecting-IP', 'POCKYMOE_CLIENT_IP_HEADER=', 'REMOTE_CODEX_CLIENT_IP_HEADER=CF-Connecting-IP']:
                with self.subTest(proxy=proxy, header=header), tempfile.TemporaryDirectory() as tmp:
                    root=Path(tmp); env,backup=root/'relay.env',root/'backup/relay.env'
                    original=(proxy+'\n'+header+'\n').encode();env.write_bytes(original)
                    if proxy.endswith('=') or header.endswith('='):
                        with self.assertRaises(configure.ConfigError): configure_env(env,backup)
                    else:
                        self.assertFalse(configure_env(env,backup))
                    self.assertEqual(env.read_bytes(),original);self.assertEqual(backup.read_bytes(),original)
        for existing,missing in [('POCKYMOE_TRUSTED_PROXY_IPS=127.0.0.1','POCKYMOE_CLIENT_IP_HEADER=CF-Connecting-IP'),('POCKYMOE_CLIENT_IP_HEADER=CF-Connecting-IP','POCKYMOE_TRUSTED_PROXY_IPS=127.0.0.1')]:
            with tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);env,backup=root/'relay.env',root/'backup/relay.env';env.write_text(existing+'\n')
                self.assertTrue(configure_env(env,backup));self.assertEqual(env.read_text(),existing+'\n'+missing+'\n')
                self.assertFalse(configure_env(env,backup));self.assertEqual(backup.read_text(),existing+'\n')

    def test_refuses_symlink_and_insecure_existing_backup(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            env = root / 'relay.env'
            env.write_text('HOST=127.0.0.1\n')
            link = root / 'link'
            link.symlink_to(env)
            with self.assertRaises(ValueError):
                configure_env(link, root / 'backup')
            backup = root / 'backup'
            backup.write_text('original')
            backup.chmod(0o644)
            with self.assertRaises(ValueError):
                configure_env(env, backup)
            self.assertNotIn('TRUSTED_PROXY', env.read_text())


class DockerPeerTests(unittest.TestCase):
    def metadata(self, mode, source='172.22.0.5', gateway='172.22.0.1'):
        return {'State': {'Running': True}, 'HostConfig': {'NetworkMode': mode},
                'NetworkSettings': {'Networks': {'bridge': {'IPAddress': source, 'Gateway': gateway}}}}

    def test_host_bridge_and_default_modes_and_gateway_are_not_guessed(self):
        self.assertEqual(configure.metadata_peers(self.metadata('host'))[0], 'host')
        for mode in ['bridge', 'default', 'private-network']:
            _, peers = configure.metadata_peers(self.metadata(mode))
            self.assertIn(ipaddress.ip_address('172.22.0.5'), peers)
            self.assertIn(ipaddress.ip_address('172.22.0.1'), peers)
        with patch.object(configure, 'container_metadata', return_value=configure.metadata_peers(self.metadata('bridge'))), patch.object(configure, 'observe_peer', return_value='172.22.0.5'):
            self.assertEqual(configure.derive_peers('test-container', [{'host': 'host.docker.internal', 'port': 18791}]), ['172.22.0.5'])
        with patch.object(configure, 'container_metadata', return_value=configure.metadata_peers(self.metadata('bridge'))), patch.object(configure, 'observe_peer', return_value='172.22.0.1'):
            self.assertEqual(configure.derive_peers('test-container', [{'host': 'host.docker.internal', 'port': 18791}]), ['172.22.0.1'])

    def test_kernel_socket_table_proves_actual_peer_including_mapped_ipv6(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            root.joinpath('tcp').write_text('header\n 0: 0100007F:4967 050016AC:9C40 01 rest\n')
            self.assertEqual(configure.observed_peers(40000, 18791, root), {ipaddress.ip_address('172.22.0.5')})
            self.assertEqual(configure.observed_peers(40001, 18791, root), set())
            root.joinpath('tcp').write_text('header\n')
            address = socket.inet_pton(socket.AF_INET6, '::ffff:172.22.0.5')
            encoded = b''.join(address[i:i+4][::-1] for i in range(0, 16, 4)).hex().upper()
            root.joinpath('tcp6').write_text(f'header\n 0: {"0"*32}:4967 {encoded}:9C40 01 rest\n')
            self.assertEqual(configure.observed_peers(40000, 18791, root), {ipaddress.ip_address('172.22.0.5')})

    def test_existing_wrong_peer_fails_before_edit_and_bridge_defaults_use_observed_peer(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); env, backup = root/'relay.env', root/'backup/relay.env'
            env.write_text('POCKYMOE_TRUSTED_PROXY_IPS=127.0.0.1\n')
            with self.assertRaisesRegex(configure.ConfigError, '^peer-not-trusted$'):
                configure.configure(env, backup, ['172.22.0.5'])
            self.assertEqual(env.read_text(), backup.read_text())
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); env, backup=root/'relay.env',root/'backup/relay.env'; env.write_text('HOST=127.0.0.1\n')
            self.assertTrue(configure.configure(env,backup,['172.22.0.5']))
            self.assertIn('POCKYMOE_TRUSTED_PROXY_IPS=172.22.0.5',env.read_text())
            self.assertEqual(backup.read_text(),'HOST=127.0.0.1\n')

    def test_observed_bridge_peer_and_host_source_are_checked_not_inferred(self):
        class Child:
            stdout = io.StringIO()
            def terminate(self): pass
            def wait(self, timeout): pass
        class Ready:
            def __enter__(self): return self
            def __exit__(self, *args): pass
            def register(self, *args): pass
            def select(self, *args): return [True]
        for mode, observed, source, allowed in [
            ('bridge', '172.22.0.5', '172.22.0.5', True),
            ('bridge', '172.22.0.1', '172.22.0.5', True),
            ('bridge', '198.51.100.25', '172.22.0.5', False),
            ('host', '127.0.0.1', '127.0.0.1', True),
            ('host', '198.51.100.25', '198.51.100.25', True),
            ('host', '198.51.100.26', '198.51.100.25', False),
        ]:
            child=Child();child.stdout=io.StringIO(json.dumps({'port':40000,'source':source})+'\n')
            with patch.object(configure.subprocess,'Popen',return_value=child), patch.object(configure.selectors,'DefaultSelector',return_value=Ready()), patch.object(configure,'observed_peers',return_value={ipaddress.ip_address(observed)}):
                if allowed:
                    self.assertEqual(configure.observe_peer('test',{'host':'host.docker.internal','port':18791},mode,{ipaddress.ip_address('172.22.0.5'),ipaddress.ip_address('172.22.0.1')}),observed)
                else:
                    with self.assertRaisesRegex(configure.ConfigError,'^peer-not-container$'):
                        configure.observe_peer('test',{'host':'host.docker.internal','port':18791},mode,{ipaddress.ip_address('172.22.0.5'),ipaddress.ip_address('172.22.0.1')})

    def test_docker_inspection_reads_network_metadata_only(self):
        result=SimpleNamespace(stdout=json.dumps(self.metadata('bridge')))
        with patch.object(configure.subprocess,'run',return_value=result) as command:
            mode,_=configure.container_metadata('test')
        self.assertEqual(mode,'bridge')
        args=command.call_args.args[0]
        self.assertIn('--format',args)
        self.assertNotIn('.Config.Env',args[args.index('--format')+1])
        self.assertIn('.NetworkSettings.Networks',args[args.index('--format')+1])

    def test_public_cli_errors_are_fixed_codes_even_if_exception_contains_private_details(self):
        output=io.StringIO()
        with patch.object(configure, 'derive_peers', side_effect=RuntimeError('private-host.example /private/path 172.22.0.5')), contextlib.redirect_stdout(output):
            with tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp); plan=root/'plan';plan.write_text('[{"host":"private-host.example","port":18791}]')
                self.assertEqual(configure.main(['--env',str(root/'env'),'--backup',str(root/'backup'),'--container','test','--upstreams',str(plan)]),1)
        self.assertEqual(output.getvalue(),'FAIL: environment-configure-failed\n')
        self.assertEqual(str(configure.ConfigError('private-host.example')), 'environment-configure-failed')



class ForwardingTests(unittest.TestCase):
    def check(self, forwarding='$proxy_add_x_forwarded_for', http='', cf=False):
        return audit.audit(dump('location / { proxy_set_header X-Forwarded-For ' + forwarding + '; proxy_pass http://127.0.0.1:18791; }', http), 'relay.example.com', cf, NETWORKS)

    def test_append_or_overwrite_keeps_forged_prefix_left_of_actual_peer(self):
        self.assertEqual(self.check(), 1)
        self.assertEqual(self.check('$remote_addr'), 1)
        # A client-controlled header without the transport remote address fails.
        for value in ['$http_x_forwarded_for', '$http_cf_connecting_ip', '"203.0.113.234"']:
            with self.subTest(value=value), self.assertRaises(audit.AuditError):
                self.check(value)

    def test_cdn_requires_edge_only_normalization_and_all_edge_ranges(self):
        with self.assertRaisesRegex(audit.AuditError, '^cdn-real-ip-missing$'):
            self.check(cf=True)
        policy = 'real_ip_header CF-Connecting-IP; set_real_ip_from 192.0.2.0/24; set_real_ip_from 2001:db8::/32;'
        self.assertEqual(self.check(http=policy, cf=True), 1)
        for invalid in [policy.replace('192.0.2.0/24', '0.0.0.0/0'), policy.replace('set_real_ip_from 2001:db8::/32;', ''), policy.replace('CF-Connecting-IP', 'X-Forwarded-For')]:
            with self.subTest(policy=invalid), self.assertRaises(audit.AuditError):
                self.check(http=invalid, cf=True)

    def test_dedicated_cf_header_does_not_require_nginx_remote_addr_rewriting(self):
        direct=dump('location / { proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for; proxy_pass http://127.0.0.1:18791; }')
        self.assertEqual(audit.audit(direct,'relay.example.com',True,NETWORKS,'CF-Connecting-IP'),1)
        explicit=dump('location / { proxy_pass_request_headers off; proxy_set_header CF-Connecting-IP $http_cf_connecting_ip; proxy_pass http://127.0.0.1:18791; }')
        self.assertEqual(audit.audit(explicit,'relay.example.com',True,NETWORKS,'CF-Connecting-IP'),1)
        for bad in [explicit.replace('$http_cf_connecting_ip','$remote_addr'),explicit.replace('proxy_set_header CF-Connecting-IP $http_cf_connecting_ip;','')]:
            with self.assertRaises(audit.AuditError):audit.audit(bad,'relay.example.com',True,NETWORKS,'CF-Connecting-IP')

    def test_resolves_actual_loaded_includes_and_npm_upstream_variables(self):
        text = '''# configuration file /etc/nginx/nginx.conf:
http { include /data/nginx/proxy_host/*.conf; }
# configuration file /data/nginx/proxy_host/1.conf:
server { server_name relay.example.com; set $server "127.0.0.1"; set $port 18791; set $forward_scheme http;
location / { include conf.d/include/proxy.conf; } }
# configuration file /etc/nginx/conf.d/include/proxy.conf:
proxy_set_header Host $host;
proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
proxy_pass $forward_scheme://$server:$port$request_uri;
'''
        self.assertEqual(audit.audit(text, 'relay.example.com', False, NETWORKS), 1)
        self.assertEqual(audit.audit('nginx: configuration test diagnostics\n'+text+'\nnginx: test successful\n', 'relay.example.com', False, NETWORKS), 1)
        with self.assertRaises(audit.AuditError):
            audit.audit(text.replace('proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;', ''), 'relay.example.com', False, NETWORKS)

    def test_header_inheritance_and_unknown_hosts_fail_closed(self):
        text = dump('proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for; location / { proxy_set_header Host $host; proxy_pass http://127.0.0.1:18791; }')
        with self.assertRaises(audit.AuditError):
            audit.audit(text, 'relay.example.com', False, NETWORKS)
        with self.assertRaises(audit.AuditError):
            audit.audit(dump('location / { proxy_pass http://127.0.0.1:18791; }'), 'other.example.com', False, NETWORKS)

    def test_public_probe_sends_forged_headers_and_detects_cdn_without_claiming_resolved_ip(self):
        class Reply:
            status = 200
            headers = {'cf-ray': 'synthetic-ray'}
            def read(self, _limit): return b'{"status":"ok"}'
            def __enter__(self): return self
            def __exit__(self, *args): pass
        class Opener:
            def open(self, request, timeout):
                self.request = request
                return Reply()
        opener = Opener()
        with patch.object(audit.urllib.request, 'build_opener', return_value=opener), patch.object(audit.socket, 'getaddrinfo', return_value=[(2, 1, 6, '', ('198.51.100.10', 443))]):
            self.assertTrue(audit.probe('https://relay.example.com', NETWORKS))
        self.assertEqual(opener.request.get_header('X-forwarded-for'), '203.0.113.234')
        self.assertEqual(opener.request.get_header('Cf-connecting-ip'), '203.0.113.235')
        Reply.headers = {}
        with patch.object(audit.urllib.request, 'build_opener', return_value=opener), patch.object(audit.socket, 'getaddrinfo', return_value=[(2, 1, 6, '', ('192.0.2.12', 443))]):
            self.assertTrue(audit.probe('https://relay.example.com', NETWORKS))
        Reply.headers = {'via': 'unidentified intermediary'}
        with patch.object(audit.urllib.request, 'build_opener', return_value=opener), self.assertRaises(audit.AuditError):
            audit.probe('https://relay.example.com', NETWORKS)

    def test_explicit_header_cli_does_not_depend_on_unverifiable_public_probe(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); conf=root/'nginx';plan=root/'plan'
            conf.write_text(dump('set $server host.docker.internal; set $port 18791; set $forward_scheme http; location / { proxy_pass $forward_scheme://${server}:$port$request_uri; }'))
            output=io.StringIO()
            with patch.object(audit,'probe',side_effect=AssertionError('must not probe')), patch.object(audit,'official_networks',side_effect=AssertionError('must not fetch')), contextlib.redirect_stdout(output):
                result=audit.main(['--nginx-dump',str(conf),'--origin','https://relay.example.com','--client-ip-header','CF-Connecting-IP','--upstreams-out',str(plan)])
            self.assertEqual(result,0)
            self.assertIn('WARN: origin-isolation-unverified',output.getvalue())
            self.assertEqual(json.loads(plan.read_text()),[{'host':'host.docker.internal','port':18791}])
            self.assertEqual(stat.S_IMODE(plan.stat().st_mode),0o600)
            conf.write_text(dump('include /missing/private-host.example;'))
            output=io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(audit.main(['--nginx-dump',str(conf),'--origin','https://relay.example.com','--client-ip-header','CF-Connecting-IP']),1)
            self.assertEqual(output.getvalue(),'FAIL: include-unresolved\n')

    def test_workflow_orders_audit_backup_append_validation_and_service_stop(self):
        root = Path(__file__).resolve().parent.parent
        workflow = (root / '.github/workflows/relay-deploy.yml').read_text()
        start = workflow.index('      - name: Replace only the Rust relay service')
        workflow = workflow[start:]
        positions = [workflow.index(text) for text in ['nginx -T', 'check-relay-forwarding.py', 'configure-relay-proxy.py', "<<'PYPREFLIGHT'", 'sudo systemctl stop "${service}"']]
        self.assertEqual(positions, sorted(positions))
        self.assertIn('test -f "${backup}/relay-data/relay.env"', workflow)
        self.assertIn('restore the original pre-change environment', workflow)
        self.assertIn('--client-ip-header CF-Connecting-IP', workflow)
        self.assertIn('--upstreams-out', workflow)
        self.assertIn('--container', workflow)
        self.assertNotIn("if ipaddress.ip_address('127.0.0.1') not in proxies", workflow)
        self.assertIn('POCKYMOE_CLIENT_IP_HEADER', workflow)


if __name__ == '__main__':
    unittest.main()
