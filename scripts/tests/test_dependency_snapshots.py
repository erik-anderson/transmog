"""Dependency snapshot publication must be complete, bounded and recoverable."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
from dependency_snapshots import replace_directory, snapshot_file

spec = importlib.util.spec_from_file_location("vendor", SCRIPTS / "vendor-boringssl-pki.py")
vendor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vendor)


class SnapshotTests(unittest.TestCase):
    @unittest.skipUnless(os.name == "nt", "requires Windows 8.3 path aliases")
    def test_replacement_accepts_short_parent_aliases(self):
        import ctypes
        from ctypes import wintypes
        get_short_path = ctypes.WinDLL("kernel32", use_last_error=True).GetShortPathNameW
        get_short_path.argtypes = [wintypes.LPCWSTR, wintypes.LPWSTR, wintypes.DWORD]
        get_short_path.restype = wintypes.DWORD
        with tempfile.TemporaryDirectory(prefix="dependency snapshot parent alias ") as temporary:
            root = Path(temporary).resolve()
            buffer = ctypes.create_unicode_buffer(32768)
            length = get_short_path(str(root), buffer, len(buffer))
            self.assertGreater(length, 0)
            self.assertLess(length, len(buffer))
            alias = Path(buffer.value)
            if alias == root:
                self.skipTest("8.3 path aliases are disabled on this volume")
            staged, destination = alias / "stage", root / "snapshot"
            staged.mkdir()
            destination.mkdir()
            (staged / "new.h").write_text("new")
            (destination / "old.h").write_text("old")
            replace_directory(staged, destination)
            self.assertEqual((destination / "new.h").read_text(), "new")
            self.assertFalse((destination / "old.h").exists())
            self.assertFalse(staged.exists())

    def test_replacement_rejects_a_different_parent(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            staged = root / "stage"
            staged.mkdir()
            other = root / "other"
            other.mkdir()
            with self.assertRaisesRegex(ValueError, "outside its parent"):
                replace_directory(staged, other / "snapshot")
            self.assertTrue(staged.is_dir())

    def test_replacement_still_rejects_linked_children(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            target = root / "target"
            target.mkdir()
            staged = root / "stage"
            try:
                staged.symlink_to(target, target_is_directory=True)
            except OSError:
                self.skipTest("directory symlinks are unavailable")
            with self.assertRaisesRegex(ValueError, "outside its parent"):
                replace_directory(staged, root / "snapshot")
            self.assertTrue(target.is_dir())

    def fixture(self, root):
        source = root / "source"
        for name, value in {"gen/sources.cmake": "set(PKI_SOURCES\n pki/verify.cc\n)\n",
                            "pki/verify.cc": '#include "verify.h"\n', "pki/verify.h": "// header\n",
                            "include/openssl/base.h": "// public\n", "LICENSE": "license"}.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(value, encoding="utf-8")
        return source

    def generate(self, source, destination):
        with patch.object(vendor.subprocess, "check_output", side_effect=["a" * 40, ""]):
            return vendor.generate_snapshot(source, destination, "a" * 40, "b" * 40)

    def test_refresh_removes_obsolete_headers_and_rejects_unlisted_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            source = self.fixture(root)
            destination = root / "snapshot"
            self.generate(source, destination)
            (destination / "include/openssl/obsolete.h").write_text("stale")
            with self.assertRaisesRegex(ValueError, "file set differs"):
                vendor.check_snapshot(destination)
            self.generate(source, destination)
            self.assertFalse((destination / "include/openssl/obsolete.h").exists())
            vendor.check_snapshot(destination)
            self.assertEqual(sorted(p.name for p in root.iterdir()), ["snapshot", "source"])

    def test_replacement_failure_restores_the_entire_old_snapshot(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            staged, destination = root / "stage", root / "snapshot"
            staged.mkdir()
            destination.mkdir()
            (destination / "old.h").write_text("old")
            (staged / "new.h").write_text("new")
            real_replace = os.replace
            def fail_publish(source, target):
                if source == staged:
                    raise OSError("publish failed")
                return real_replace(source, target)
            with patch("dependency_snapshots.os.replace", side_effect=fail_publish):
                with self.assertRaisesRegex(OSError, "publish failed"):
                    replace_directory(staged, destination)
            self.assertEqual((destination / "old.h").read_text(), "old")
            self.assertFalse((destination / "new.h").exists())
            self.assertEqual(sorted(p.name for p in root.iterdir()), ["snapshot", "stage"])

    def test_invalid_upstream_target_leaves_existing_snapshot_unchanged(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            source = self.fixture(root)
            destination = root / "snapshot"
            self.generate(source, destination)
            old = (destination / "manifest.json").read_bytes()
            (source / "gen/sources.cmake").write_text("set(PKI_SOURCES\n ../outside.cc\n)\n")
            with self.assertRaisesRegex(ValueError, "Invalid snapshot path"):
                self.generate(source, destination)
            self.assertEqual((destination / "manifest.json").read_bytes(), old)

    def test_cargo_marker_is_allowed_but_modified_source_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            source = self.fixture(root)
            destination = root / "snapshot"
            with patch.object(vendor.subprocess, "check_output", side_effect=["a" * 40, "?? .cargo-ok\n"]):
                vendor.generate_snapshot(source, destination, "a" * 40, "b" * 40)
            old = (destination / "manifest.json").read_bytes()
            with patch.object(vendor.subprocess, "check_output", side_effect=["a" * 40, " M pki/verify.cc\n"]):
                with self.assertRaisesRegex(ValueError, "must be clean"):
                    vendor.generate_snapshot(source, destination, "a" * 40, "b" * 40)
            self.assertEqual((destination / "manifest.json").read_bytes(), old)

    def test_receipt_cannot_hash_paths_outside_the_snapshot(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            for name in ("../outside", "C:/outside", "pki\\outside", "/outside"):
                with self.subTest(name=name), self.assertRaises(ValueError):
                    snapshot_file(root, name)
            source = self.fixture(root)
            destination = root / "snapshot"
            self.generate(source, destination)
            manifest = json.loads((destination / "manifest.json").read_text())
            manifest["files"]["../outside"] = hashlib.sha256(b"").hexdigest()
            (destination / "manifest.json").write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "Invalid snapshot path"):
                vendor.check_snapshot(destination)


if __name__ == "__main__":
    unittest.main()
