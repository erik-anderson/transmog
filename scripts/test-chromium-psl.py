"""Offline regression checks for the Chromium PSL maintenance script."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import unittest

spec = importlib.util.spec_from_file_location("psl", Path(__file__).with_name("update-chromium-psl.py"))
psl = importlib.util.module_from_spec(spec)
spec.loader.exec_module(psl)


class SnapshotTests(unittest.TestCase):
    def test_snapshot_is_reproducible_and_check_is_read_only(self):
        before = {path.name: path.read_bytes() for path in psl.DEST.iterdir() if path.is_file()}
        subprocess.run([sys.executable, str(Path(psl.__file__)), "--check"], check=True)
        after = {path.name: path.read_bytes() for path in psl.DEST.iterdir() if path.is_file()}
        self.assertEqual(before, after)
        manifest = json.loads(before["snapshot.json"])
        self.assertIn(manifest["chromiumVersion"], manifest["source"])

    def test_generated_rules_exclude_private_domains_and_preserve_exceptions(self):
        rules = (psl.DEST / "icann-rules.txt").read_text().splitlines()
        self.assertIn("co.uk", rules)
        self.assertIn("*.kawasaki.jp", rules)
        self.assertIn("!city.kawasaki.jp", rules)
        self.assertNotIn("appspot.com", rules)
        # A-label conversion must not use IDNA2003's sharp-s mapping.
        data = ("// ===BEGIN ICANN DOMAINS===\n" + "\n".join(f"r{i}.com" for i in range(1000))
                + "\n!faß.de\n// ===END ICANN DOMAINS===\n").encode()
        self.assertIn(b"!xn--fa-hia.de\n", psl.generate_rules(data))

    def test_rejects_unexpected_source_and_moving_versions(self):
        for data in [b"<html>not a list</html>", b"// ===BEGIN ICANN DOMAINS===\ncom\n// ===END ICANN DOMAINS==="]:
            with self.assertRaises(ValueError):
                psl.generate_rules(data)
        result = subprocess.run([sys.executable, str(Path(psl.__file__)), "--chromium-version", "main"], capture_output=True)
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
