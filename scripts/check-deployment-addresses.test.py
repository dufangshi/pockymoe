#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('deployment', Path(__file__).with_name('check-deployment-addresses.py'))
deployment = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deployment)


class DeploymentAddressTest(unittest.TestCase):
    env = {'POCKYMOE_RELAY_ORIGIN': 'https://relay.example.com', 'POCKYMOE_LEGACY_RELAY_ORIGIN': 'https://legacy.example.com', 'POCKYMOE_WEB_DOMAIN': 'app.example.com'}

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
