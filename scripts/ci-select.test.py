import importlib.util
import unittest
from pathlib import Path
from unittest.mock import patch
spec = importlib.util.spec_from_file_location("selection", Path(__file__).with_name("ci-select.py"))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)

class SelectionTests(unittest.TestCase):
    def test_json_fields_and_deletions(self):
        a = '{"version":"1.0.0","dependencies":{"a":"1"}}'
        self.assertEqual(m.normalized_version("package.json", a), m.normalized_version("package.json", a.replace("1.0.0", "1.0.1")))
        self.assertNotEqual(m.normalized_version("package.json", a), m.normalized_version("package.json", '{"version":"1.0.1"}'))
        self.assertNotEqual(m.normalized_version("package.json", a), m.normalized_version("package.json", a.replace('"a":"1"', '"a":"2"')))

    def test_cargo_only_owned_version_fields(self):
        a = '[workspace.package]\nversion = "1.0.0"\n[dependencies]\nother = "2"\n'
        self.assertEqual(m.normalized_version("Cargo.toml", a), m.normalized_version("Cargo.toml", a.replace("1.0.0", "1.0.1")))
        self.assertNotEqual(m.normalized_version("Cargo.toml", a), m.normalized_version("Cargo.toml", a.replace('other = "2"', 'other = "3"')))
        lock = '[[package]]\nname = "pockymoe"\nversion = "1.0.0"\n[[package]]\nname = "external"\nversion = "2.0.0"\n'
        self.assertEqual(m.normalized_version("Cargo.lock", lock), m.normalized_version("Cargo.lock", lock.replace("1.0.0", "1.0.1")))
        self.assertNotEqual(m.normalized_version("Cargo.lock", lock), m.normalized_version("Cargo.lock", lock.replace("2.0.0", "2.0.1")))

    def test_pin_and_unknown_paths(self):
        self.assertTrue(m.select(["thread-ui.sha"])["web"])
        self.assertTrue(m.select(["unknown-input"])["rust"])
        self.assertFalse(any(m.select(["docs/example.md"])[flag] for flag in m.FLAGS))
        self.assertFalse(any(m.select(["Cargo.toml", "Cargo.lock", "package.json"], True)[flag] for flag in m.FLAGS))

if __name__ == "__main__":
    unittest.main()
