#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('deployment', Path(__file__).with_name('check-deployment-addresses.py'))
deployment = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deployment)


class DeploymentAddressTest(unittest.TestCase):
    env = {'POCKYMOE_RELAY_ORIGIN': 'https://relay.example.com', 'POCKYMOE_LEGACY_RELAY_ORIGIN': 'https://legacy.example.com', 'POCKYMOE_WEB_DOMAIN': 'app.example.com'}

    def test_masks_raw_secrets_and_the_hostnames_used_by_ssh_and_curl(self):
        masked = deployment.mask_values(self.env)
        for value in ['https://relay.example.com', 'relay.example.com', 'https://legacy.example.com', 'legacy.example.com', 'app.example.com', 'https://app.example.com', 'wss://relay.example.com']:
            self.assertIn(value, masked)

    def test_workflows_require_secrets_and_mask_before_address_use(self):
        root = Path(__file__).resolve().parent.parent
        for workflow in ['pages-deploy.yml', 'relay-deploy.yml']:
            text = (root / '.github/workflows' / workflow).read_text()
            for name in self.env:
                self.assertIn('secrets.' + name, text)
                self.assertNotIn('vars.' + name, text)
            self.assertIn('scripts/check-deployment-addresses.py --mask', text)
            self.assertLess(text.index('scripts/check-deployment-addresses.py --mask'), text.index('curl ' if workflow.startswith('relay') else 'Read shared UI pin'))

    def test_relay_host_secrets_are_required_validated_and_masked(self):
        env = dict(self.env, POCKYMOE_RELAY_SSH_FINGERPRINT='SHA256:' + 'a' * 43,
                   POCKYMOE_RELAY_HOSTNAME='relay-host.example', REMOTE_CODEX_RELAY_DEPLOY_HOST='192.0.2.10',
                   REMOTE_CODEX_RELAY_DEPLOY_USER='deploy', REMOTE_CODEX_RELAY_DEPLOY_PASSWORD='synthetic-password',
                   REMOTE_CODEX_RELAY_DEPLOY_PORT='22')
        deployment.validate(env, relay_host=True)
        for name in set(env) - set(self.env):
            missing = dict(env)
            missing.pop(name)
            with self.subTest(name=name), self.assertRaises(ValueError):
                deployment.validate(missing, relay_host=True)
        for name in ['POCKYMOE_RELAY_SSH_FINGERPRINT', 'POCKYMOE_RELAY_HOSTNAME']:
            self.assertIn(env[name], deployment.mask_values(env))
        root = Path(__file__).resolve().parent.parent
        workflow = (root / '.github/workflows/relay-deploy.yml').read_text()
        self.assertIn('--mask --relay-host', workflow)
        self.assertEqual(workflow.count('fingerprint: ${{ secrets.POCKYMOE_RELAY_SSH_FINGERPRINT }}'), 3)
        self.assertIn('test "$(hostname)" = "${POCKYMOE_RELAY_HOSTNAME}"', workflow)
        self.assertIn('POCKYMOE_TRUSTED_PROXY_IPS', workflow)

    def test_expected_configuration(self):
        deployment.validate(self.env)

    def test_missing_or_non_origin_values_fail(self):
        for name in self.env:
            env = dict(self.env)
            env.pop(name)
            with self.subTest(name=name), self.assertRaises(ValueError):
                deployment.validate(env)
        for value in ['http://relay.example.com', 'https://relay.example.com/', 'https://relay.example.com/path', 'https://user@relay.example.com', 'https://relay.example.com?x=1']:
            with self.subTest(value=value), self.assertRaises(ValueError):
                deployment.validate(dict(self.env, POCKYMOE_RELAY_ORIGIN=value))


if __name__ == '__main__':
    unittest.main()
