import importlib.util
import unittest
from pathlib import Path
spec = importlib.util.spec_from_file_location("validation", Path(__file__).with_name("runtime-validation.py"))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)

class AffectedTests(unittest.TestCase):
    packages = [
        {"name":"pockymoe-runtime", "manifest_path":"/repo/crates/runtime/Cargo.toml", "dependencies":[]},
        {"name":"pockymoe-supervisor", "manifest_path":"/repo/crates/supervisor/Cargo.toml", "dependencies":[{"name":"pockymoe-runtime"}]},
        {"name":"pockymoe-relay", "manifest_path":"/repo/crates/relay/Cargo.toml", "dependencies":[]},
    ]
    def select(self, *paths):
        return m.affected(paths, self.packages, Path("/repo"))
    def test_consumers_and_sensitive_paths_use_the_same_rule(self):
        self.assertEqual(self.select("crates/runtime/src/update.rs"), ["pockymoe-runtime", "pockymoe-supervisor"])
        self.assertEqual(self.select("crates/relay/src/auth.rs"), ["pockymoe-relay"])
        self.assertEqual(self.select("crates/supervisor/migrations/change.sql"), ["pockymoe-supervisor"])
    def test_non_rust_and_unknown_inputs(self):
        self.assertEqual(self.select("docs/ci.md", "apps/supervisor-web/src/main.ts"), [])
        self.assertEqual(len(self.select("new-build-input")), 3)
        self.assertEqual(len(self.select("Cargo.lock")), 3)

if __name__ == "__main__":
    unittest.main()
