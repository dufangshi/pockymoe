#!/usr/bin/env python3
import importlib.util
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
            self.assertTrue(configure.configure(env, backup))
            expected = original + b'\nPOCKYMOE_TRUSTED_PROXY_IPS=127.0.0.1\n'
            self.assertEqual(env.read_bytes(), expected)
            self.assertEqual(backup.read_bytes(), original)
            self.assertEqual(stat.S_IMODE(backup.stat().st_mode), 0o600)
            self.assertEqual(stat.S_IMODE(backup.parent.stat().st_mode), 0o700)
            self.assertEqual(stat.S_IMODE(env.stat().st_mode), 0o640)
            self.assertFalse(configure.configure(env, backup))
            self.assertEqual(env.read_bytes(), expected)
            self.assertEqual(backup.read_bytes(), original)

    def test_preserves_all_existing_values_including_legacy_and_explicit_empty(self):
        for line in ['POCKYMOE_TRUSTED_PROXY_IPS=127.0.0.1,10.0.0.1', 'POCKYMOE_TRUSTED_PROXY_IPS=', 'REMOTE_CODEX_TRUSTED_PROXY_IPS=127.0.0.1', '# POCKYMOE_TRUSTED_PROXY_IPS=ignored']:
            with self.subTest(line=line), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                env, backup = root / 'relay.env', root / 'backup/relay.env'
                env.write_text(line + '\n')
                changed = configure.configure(env, backup)
                self.assertEqual(changed, line.startswith('#'))
                self.assertEqual(backup.read_text(), line + '\n')
                if not changed:
                    self.assertEqual(env.read_text(), line + '\n')

    def test_refuses_symlink_and_insecure_existing_backup(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            env = root / 'relay.env'
            env.write_text('HOST=127.0.0.1\n')
            link = root / 'link'
            link.symlink_to(env)
            with self.assertRaises(ValueError):
                configure.configure(link, root / 'backup')
            backup = root / 'backup'
            backup.write_text('original')
            backup.chmod(0o644)
            with self.assertRaises(ValueError):
                configure.configure(env, backup)
            self.assertNotIn('TRUSTED_PROXY', env.read_text())


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
        with self.assertRaisesRegex(audit.AuditError, 'Cloudflare detected without'):
            self.check(cf=True)
        policy = 'real_ip_header CF-Connecting-IP; set_real_ip_from 192.0.2.0/24; set_real_ip_from 2001:db8::/32;'
        self.assertEqual(self.check(http=policy, cf=True), 1)
        for invalid in [policy.replace('192.0.2.0/24', '0.0.0.0/0'), policy.replace('set_real_ip_from 2001:db8::/32;', ''), policy.replace('CF-Connecting-IP', 'X-Forwarded-For')]:
            with self.subTest(policy=invalid), self.assertRaises(audit.AuditError):
                self.check(http=invalid, cf=True)

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

    def test_workflow_orders_audit_backup_append_validation_and_service_stop(self):
        root = Path(__file__).resolve().parent.parent
        workflow = (root / '.github/workflows/relay-deploy.yml').read_text()
        start = workflow.index('      - name: Replace only the Rust relay service')
        workflow = workflow[start:]
        positions = [workflow.index(text) for text in ['nginx -T', 'check-relay-forwarding.py', 'configure-relay-proxy.py', "<<'PYPREFLIGHT'", 'sudo systemctl stop "${service}"']]
        self.assertEqual(positions, sorted(positions))
        self.assertIn('test -f "${backup}/relay-data/relay.env"', workflow)
        self.assertIn('restore the original pre-change environment', workflow)


if __name__ == '__main__':
    unittest.main()
