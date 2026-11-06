"""Exercise discovery and safe retention across old and new V8 cache layouts."""
import hashlib
import json
import os
from pathlib import Path
import sys
import subprocess
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from build_cache import PIN, cargo_locks, file_lock
from disk_cleanup import clean_disk
from v8_cache import cache_lease, maintain_v8, source_name


class V8CacheTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.vendor = self.root / "third_party/rusty-v8"
        self.vendor.mkdir(parents=True)
        self.pin = {"version": "152.2.0", "archive_sha256": "a" * 64,
                    "icuData": {"filteredSha256": "b" * 64}}
        (self.vendor / "source.json").write_text(json.dumps(self.pin))
        (self.vendor / "native.patch").write_bytes(b"reviewed patch")

    def old(self, directory, number=0):
        for path in [directory, *directory.rglob("*")]:
            stamp = time.time() - 86400 * 5 + number
            os.utime(path, (stamp, stamp))
        return directory

    def source(self, number=0, *, legacy=False, current=False):
        inputs = {"format": 1, "archiveSha256": self.pin["archive_sha256"],
                  "nativePatchSha256": hashlib.sha256(b"reviewed patch").hexdigest() if current else f"{number:064x}",
                  "icuDataSha256": self.pin["icuData"]["filteredSha256"]}
        name = "152.2.0-" + "a" * 12 if legacy else source_name("152.2.0", inputs)
        parent = self.root / ".tools/rusty-v8" / ("" if legacy else "sources")
        path = parent / name
        path.mkdir(parents=True)
        (path / ".transmog-source.json").write_text(json.dumps({"archive_sha256": "a" * 64, "files": {"source.cc": "c" * 64}}))
        if not legacy:
            (path / ".transmog-inputs.json").write_text(json.dumps(inputs))
        (path / "source.cc").write_bytes(b"rebuildable source")
        return self.old(path, number)

    def native(self, number, *, target="target", triple="x86_64-pc-windows-msvc"):
        path = self.root / target / "native-v8" / triple / f"{number:016x}"
        gn = path / "gn_out"
        gn.mkdir(parents=True)
        for name in ("args.gn", "build.ninja"):
            (gn / name).write_text("native configuration")
        (gn / "project.json").write_text(json.dumps({"targets": {"//:rusty_v8": {}}}))
        (path / "v8.fslock").touch()
        (gn / "rusty_v8.lib").write_bytes(b"native archive")
        return self.old(path, number)

    def test_combined_cleanup_discovers_legacy_sources_and_shared_native_variants(self):
        current = self.source(current=True)
        legacy, obsolete = self.source(legacy=True), self.source(1)
        native = [self.native(n) for n in range(1, 4)]
        arm = [self.native(n, triple="aarch64-pc-windows-msvc") for n in range(1, 3)]
        tool = self.root / ".tools/rusty-v8/tools/x86_64-pc-windows-msvc/clang.exe"
        tool.parent.mkdir(parents=True)
        tool.write_bytes(b"keep installed tools")
        report = clean_disk(self.root, include_release=True)
        candidates = {row["path"] for row in report["v8Caches"]["candidates"]}
        for path in [legacy, obsolete, *native[:2], arm[0]]:
            self.assertIn(str(path.relative_to(self.root)), candidates)
            self.assertTrue(path.exists())
        applied = clean_disk(self.root, apply=True, include_release=True)
        self.assertFalse(applied["errors"])
        self.assertTrue(all(p.exists() for p in [current, native[-1], arm[-1], tool]))
        self.assertTrue(all(not p.exists() for p in [legacy, obsolete, *native[:2], arm[0]]))
        self.assertEqual(clean_disk(self.root, apply=True, include_release=True)["removedLogicalBytes"], 0)

    def test_default_preserves_shared_production_cache_and_conservative_keeps_three(self):
        self.source(current=True)
        variants = [self.native(n) for n in range(1, 5)]
        report = maintain_v8(self.root, apply=True)
        self.assertEqual(report["removedLogicalBytes"], 0)
        self.assertTrue(all(p.exists() for p in variants))
        report = maintain_v8(self.root, apply=True, deep=False, include_release=True)
        self.assertFalse(variants[0].exists())
        self.assertTrue(all(p.exists() for p in variants[1:]))
        self.assertGreater(report["removedLogicalBytes"], 0)

    def test_native_preparation_tools_and_cargo_locks_protect_active_caches(self):
        self.source(current=True)
        variants = [self.native(n) for n in range(1, 3)]
        profile = self.root / "target/release"
        (profile / "deps").mkdir(parents=True)
        (profile / ".fingerprint").mkdir()
        tool_lock = self.root / ".tools/rusty-v8/tools/x86_64-pc-windows-msvc/toolchain.fslock"
        tool_lock.parent.mkdir(parents=True)
        for lease in (cache_lease(self.root), cargo_locks(profile, self.root), file_lock(tool_lock, self.root)):
            with lease:
                self.assertEqual(maintain_v8(self.root, apply=True, include_release=True)["status"], "busy")
                self.assertTrue(all(p.exists() for p in variants))

    def test_pins_unknown_layouts_and_unverified_checkout_preserve_data(self):
        current = self.source(current=True)
        legacy = self.source(legacy=True)
        pinned = self.native(1)
        self.native(2)
        (pinned / "gn_out" / PIN).touch()
        unknown = self.root / "target/native-v8/x86_64-pc-windows-msvc/0000000000000003"
        unknown.mkdir()
        (unknown / "personal.txt").write_text("unknown")
        (self.vendor / "source.json").unlink()
        report = maintain_v8(self.root, apply=True, include_release=True)
        self.assertTrue(all(p.exists() for p in [current, legacy, pinned, unknown]))
        self.assertTrue(any(row["reason"] == "unknown layout" for row in report["retained"]))
        (self.root / "target" / PIN).touch()
        self.assertEqual(maintain_v8(self.root, apply=True, include_release=True)["removedLogicalBytes"], 0)

    def test_registered_sdk_cache_and_interrupted_deletion_are_recovered(self):
        self.source(current=True)
        old = self.native(1, target=".local/sdk/rust-target")
        latest = self.native(2, target=".local/sdk/rust-target")
        trash = old.parent / ".transmog-cache-trash" / ("a" * 32)
        trash.mkdir(parents=True)
        (trash / "old.obj").write_bytes(b"interrupted deletion")
        report = maintain_v8(self.root, apply=True, include_release=True)
        self.assertFalse(report["errors"])
        self.assertFalse(old.exists())
        self.assertFalse(trash.exists())
        self.assertTrue(latest.exists())
        self.assertGreater(report["removedLogicalBytes"], 0)

    def test_linked_native_variant_is_preserved_without_traversal(self):
        self.source(current=True)
        old, latest = self.native(1), self.native(2)
        outside = self.root / "personal"
        outside.mkdir()
        evidence = outside / "keep.txt"
        evidence.write_text("keep")
        link = old.parent / ("f" * 16)
        try:
            link.symlink_to(outside, target_is_directory=True)
        except OSError:
            if os.name != "nt":
                raise
            result = subprocess.run(["cmd.exe", "/d", "/c", "mklink", "/J", str(link), str(outside)],
                                    capture_output=True, text=True)
            if result.returncode:
                self.skipTest("Windows sandbox does not permit directory links")
        report = maintain_v8(self.root, apply=True, include_release=True)
        self.assertFalse(old.exists())
        self.assertTrue(latest.exists())
        self.assertTrue(evidence.exists())
        self.assertTrue(link.exists())
        self.assertTrue(any(row["reason"] == "unreadable or unsafe" for row in report["retained"]))


if __name__ == "__main__":
    unittest.main()
