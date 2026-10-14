"""The public single-script installer must contain the sources we actually ship."""
from pathlib import Path
import shutil
import tempfile
import unittest

import bundle_setup


class BundleTests(unittest.TestCase):
    def test_checked_in_bundle_matches_every_source(self):
        bundle_setup.check(Path(__file__).parent)

    def test_editing_any_bundled_source_fails_check(self):
        source = Path(__file__).parent
        for name in bundle_setup.FILES:
            with self.subTest(file=name), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                for filename in (*bundle_setup.FILES, bundle_setup.NAME):
                    shutil.copyfile(source / filename, directory / filename)
                with (directory / name).open("ab") as output:
                    output.write(b"\n")
                with self.assertRaisesRegex(ValueError, "Stale Treer setup bundle"):
                    bundle_setup.check(directory)


if __name__ == "__main__":
    unittest.main()
