"""A coordinated roll prepares pins and source data without activating profiles."""
import importlib.util
from pathlib import Path
import sys
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
spec = importlib.util.spec_from_file_location("roll", SCRIPTS / "roll-chromium-dependencies.py")
roll = importlib.util.module_from_spec(spec)
spec.loader.exec_module(roll)


class RollTests(unittest.TestCase):
    def test_exported_patch_round_trips_new_deleted_and_binary_snapshot_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            candidate, applied = root / "candidate", root / "applied"
            candidate.mkdir()
            def git(directory, *arguments):
                return subprocess.run(["git", "-C", str(directory), *arguments], capture_output=True, check=True)
            git(candidate, "init", "--quiet")
            git(candidate, "config", "core.autocrlf", "false")
            (candidate / "obsolete.h").write_bytes(b"old")
            (candidate / "roots.pb").write_bytes(b"\x00old binary\xff")
            git(candidate, "add", ".")
            git(candidate, "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "--quiet", "-m", "baseline")
            git(root, "clone", "--quiet", str(candidate), str(applied))
            git(applied, "config", "core.autocrlf", "false")
            (candidate / "obsolete.h").unlink()
            (candidate / "new.h").write_bytes(b"new\n")
            (candidate / "roots.pb").write_bytes(b"\x00new binary\xff")
            summary, patch_text = roll.export_candidate(candidate, None)
            self.assertIn("new.h", summary)
            self.assertIn("obsolete.h", summary)
            patch_file = root / "changes.patch"
            patch_file.write_text(patch_text, encoding="utf-8", newline="\n")
            git(applied, "apply", str(patch_file))
            self.assertFalse((applied / "obsolete.h").exists())
            for name in ("new.h", "roots.pb"):
                self.assertEqual((applied / name).read_bytes(), (candidate / name).read_bytes())

    def test_all_eight_family_pins_change_without_modifying_other_dependencies(self):
        original = (roll.ROOT / "Cargo.toml").read_text(encoding="utf-8")
        changed = roll.update_cargo(original, "5.3.0", "c" * 40)
        import tomllib
        parsed = tomllib.loads(changed)
        before = tomllib.loads(original)
        for name in roll.FAMILY:
            dependency = parsed["workspace"]["dependencies"][name]
            self.assertEqual(dependency["version"] if isinstance(dependency, dict) else dependency, "=5.3.0")
            self.assertEqual(parsed["patch"]["crates-io"][name]["rev"], "c" * 40)
        for name in set(before["workspace"]["dependencies"]) - set(roll.FAMILY):
            self.assertEqual(parsed["workspace"]["dependencies"][name], before["workspace"]["dependencies"][name])
        with self.assertRaises(ValueError):
            roll.update_cargo(original, "5.3.0", "main")

    def test_missing_family_pin_is_an_error_instead_of_a_partial_roll(self):
        original = (roll.ROOT / "Cargo.toml").read_text(encoding="utf-8")
        missing = "\n".join(line for line in original.splitlines() if not line.startswith("tokio-boring ="))
        with self.assertRaisesRegex(ValueError, "missing a Boring family pin"):
            roll.update_cargo(missing, "5.3.0", "c" * 40)

    def test_root_store_downloads_are_bounded_and_reject_non_source_responses(self):
        class Response:
            def __init__(self, data): self.data = data
            def __enter__(self): return self
            def __exit__(self, *unused): pass
            def read(self, limit): return self.data[:limit]
        import base64
        with patch.object(roll.urllib.request, "urlopen", return_value=Response(base64.b64encode(b"public source"))):
            self.assertEqual(roll.fetch_chromium("a" * 40, "LICENSE"), b"public source")
        for data in (b"", b"not-base64!", b"x" * (8 * 1024 * 1024 + 1)):
            with patch.object(roll.urllib.request, "urlopen", return_value=Response(data)):
                with self.assertRaises(ValueError):
                    roll.fetch_chromium("a" * 40, "LICENSE")


if __name__ == "__main__":
    unittest.main()
