"""Cache retention, path/lock safety, and real Cargo rebuild regressions."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import build_cache
from repo_tools import ROOT, command, development_environment, executable


class CacheTests(unittest.TestCase):
    def setUp(self):
        self.owner = tempfile.TemporaryDirectory()
        self.addCleanup(self.owner.cleanup)
        self.root = Path(self.owner.name)
        self.profile = self.root / "target/debug"
        (self.profile / "deps").mkdir(parents=True)
        (self.profile / ".fingerprint").mkdir()
        self.now = time.time()

    def variant(self, number, *, age=3, kind="lib-example", package="example"):
        digest = f"{number:016x}"
        fingerprint = self.profile / ".fingerprint" / f"{package}-{digest}"
        fingerprint.mkdir()
        metadata = fingerprint / (kind + ".json")
        metadata.write_text(json.dumps({"rustc": 1, "target": 1, "profile": 1, "deps": []}))
        outputs = [self.profile / "deps" / f"{prefix}example-{digest}{suffix}"
                   for prefix, suffix in [("lib", ".rlib"), ("", ".pdb"), ("", ".d")]]
        for path in outputs:
            path.write_bytes(b"cache")
        for path in [metadata, *outputs]:
            timestamp = self.now - age * build_cache.DAY + number
            os.utime(path, (timestamp, timestamp))
        return fingerprint, outputs

    def maintain(self, **kwargs):
        return build_cache.maintain(self.root, min_free_gib=0, now=self.now, **kwargs)

    def test_preview_and_retention_remove_complete_old_groups_only(self):
        entries = [self.variant(i) for i in range(5)]
        unrelated = self.profile / "deps/orphan-1111111111111111.pdb"
        unrelated.write_bytes(b"unpaired")
        release = self.root / "target/release/user-output.txt"
        release.parent.mkdir()
        release.write_text("retained")
        preview = self.maintain()
        self.assertEqual(preview["variants"], 2)
        self.assertTrue(all(path.exists() for fp, files in entries for path in [fp, *files]))
        result = self.maintain(apply=True)
        self.assertEqual(result["variants"], 2)
        self.assertFalse(result["errors"])
        self.assertTrue(all(not p.exists() for fp, files in entries[:2] for p in [fp, *files]))
        self.assertTrue(all(p.exists() for fp, files in entries[2:] for p in [fp, *files]))
        self.assertTrue(unrelated.exists())
        self.assertEqual(release.read_text(), "retained")

    def test_recent_variants_daily_cadence_and_emergency_space(self):
        for number in range(5):
            self.variant(number, age=0.5)
        self.assertEqual(self.maintain(apply=True, daily=True)["variants"], 0)
        self.assertEqual(self.maintain(apply=True, daily=True)["status"], "recent")
        with patch.object(build_cache.shutil, "disk_usage", return_value=type("Disk", (), {"free": 0})()):
            result = build_cache.maintain(self.root, apply=True, daily=True, now=self.now)
        self.assertEqual(result["variants"], 2)

    def test_metadata_only_checks_do_not_displace_linked_builds(self):
        linked = [self.variant(i) for i in range(5)]
        for number in range(5, 8):
            _, outputs = self.variant(number)
            for path in outputs[:2]:
                path.unlink()
            (self.profile / "deps" / f"libexample-{number:016x}.rmeta").write_bytes(b"metadata")
        result = self.maintain(apply=True)
        self.assertEqual(result["variants"], 2)
        self.assertTrue(all(p.exists() for fp, files in linked[2:] for p in [fp, *files]))

    def test_locked_profile_is_skipped_without_advancing_daily_stamp(self):
        for number in range(5):
            self.variant(number)
        with build_cache.cargo_locks(self.profile, self.root) as acquired:
            self.assertTrue(acquired)
            self.assertEqual(self.maintain(apply=True, daily=True)["status"], "busy")
        self.assertFalse((self.profile / ".transmog-cache-maintenance.json").exists())

    def test_daily_command_detection_and_explicit_opt_out(self):
        self.assertTrue(build_cache.is_build_command(["+stable", "--offline", "--locked", "build"]))
        self.assertTrue(build_cache.is_build_command(["--color", "never", "check"]))
        self.assertFalse(build_cache.is_build_command(["metadata", "--no-deps"]))
        self.assertFalse(build_cache.is_build_command(["fmt"]))
        with patch.object(build_cache, "maintain") as maintain:
            build_cache.automatic({"TRANSMOG_BUILD_CACHE_TRIM": "0"}, root=self.root)
            build_cache.automatic({"CI": "true"}, root=self.root)
            build_cache.automatic({}, root=self.root, arguments=["build", "--target-dir", "elsewhere"])
            maintain.assert_not_called()

    def test_failed_move_rolls_back_artifacts_and_fingerprint(self):
        entries = [self.variant(i) for i in range(5)]
        rename = Path.rename
        def refuse_pdb(path, destination):
            if path.suffix == ".pdb" and path.parent.name == "deps":
                raise PermissionError("fixture loaded symbol file")
            return rename(path, destination)
        with patch.object(Path, "rename", refuse_pdb):
            result = self.maintain(apply=True)
        self.assertEqual(len(result["errors"]), 2)
        self.assertTrue(all(path.exists() for fp, files in entries for path in [fp, *files]))

    def test_unknown_sidecars_keep_the_entire_group(self):
        for number in range(5):
            self.variant(number)
            (self.profile / "deps" / f"example-{number:016x}.unknown").write_bytes(b"unknown")
        self.assertEqual(self.maintain(apply=True)["variants"], 0)

    def test_incremental_retention_keeps_recent_variants(self):
        self.variant(1)
        incremental = self.profile / "incremental"
        incremental.mkdir()
        entries = []
        for number in range(5):
            directory = incremental / f"example-{number}abc"
            directory.mkdir()
            (directory / "cache.bin").write_bytes(b"incremental")
            timestamp = self.now - 3 * build_cache.DAY + number
            os.utime(directory / "cache.bin", (timestamp, timestamp))
            os.utime(directory, (timestamp, timestamp))
            entries.append(directory)
        result = self.maintain(apply=True)
        self.assertEqual(result["variants"], 2)
        self.assertFalse(any(directory.exists() for directory in entries[:2]))
        self.assertTrue(all(directory.exists() for directory in entries[2:]))

    def test_external_targets_and_links_are_rejected(self):
        with self.assertRaises(ValueError):
            build_cache.maintain(self.root, self.root.parent / "outside")
        external = self.root / "outside"
        external.mkdir()
        link = self.profile / "deps/link-1111111111111111.rlib"
        try:
            link.symlink_to(external / "missing")
        except OSError:
            if os.name != "nt":
                raise
            result = subprocess.run(["cmd.exe", "/d", "/c", "mklink", "/J", str(link), str(external)],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
        with self.assertRaises(ValueError):
            build_cache.checked(link, self.root)

    def test_parent_traversal_is_rejected_before_cache_mutation(self):
        entries = [self.variant(i) for i in range(5)]
        nested_root = self.root / "repo"
        nested_root.mkdir()
        targets = (nested_root / ".." / "target", nested_root / ".." / "repo" / ".." / "target")
        for target in targets:
            # The fixture stays inside this test's temporary owner even though
            # it resolves outside the repository passed to maintenance.
            self.assertTrue(target.resolve().is_relative_to(self.root))
            with self.subTest(target=target), self.assertRaises(ValueError):
                build_cache.maintain(nested_root, target, apply=True, min_age_hours=0, min_free_gib=0)
        with self.assertRaises(ValueError):
            build_cache.maintain(self.root, self.root / "target" / ".." / "target", apply=True)
        self.assertTrue(all(p.exists() for fp, files in entries for p in [fp, *files]))
        self.assertFalse(any((self.profile / name).exists() for name in build_cache.LOCKS))
        self.assertFalse((self.profile / ".transmog-cache-maintenance.json").exists())

    def test_automatic_cleanup_preserves_parent_shared_target(self):
        entries = [self.variant(i) for i in range(5)]
        nested_root = self.root / "repo"
        nested_root.mkdir()
        for target in ("../target", str(nested_root / ".." / "target")):
            with self.subTest(target=target):
                build_cache.automatic({"CARGO_TARGET_DIR": target}, root=nested_root, arguments=["build"])
                self.assertTrue(all(p.exists() for fp, files in entries for p in [fp, *files]))
        self.assertFalse(any((self.profile / name).exists() for name in build_cache.LOCKS))
        self.assertFalse((self.profile / ".transmog-cache-maintenance.json").exists())

    def test_malformed_fingerprints_preserve_artifacts_and_allow_other_cleanup(self):
        values = (None, 0, True, [], ["rustc", "target", "profile", "deps"],
                  "rustc target profile deps", {}, {"rustc": 1})
        payloads = [*map(json.dumps, values), "{broken", "[" * 2000 + "0" + "]" * 2000]
        malformed = []
        for number, payload in enumerate(payloads):
            fingerprint, files = self.variant(number, package="invalid")
            metadata = fingerprint / "lib-example.json"
            metadata.write_text(payload)
            old = self.now - 3 * build_cache.DAY + number
            os.utime(metadata, (old, old))
            malformed.extend([fingerprint, *files])
        valid = [self.variant(i, package="valid") for i in range(100, 105)]
        result = self.maintain(apply=True)
        self.assertEqual(result["variants"], 2)
        self.assertFalse(result["errors"])
        self.assertTrue(all(p.exists() for p in malformed))
        self.assertTrue(all(not p.exists() for fp, files in valid[:2] for p in [fp, *files]))
        self.assertTrue(all(p.exists() for fp, files in valid[2:] for p in [fp, *files]))


class CargoAcceptance(unittest.TestCase):
    def test_malformed_fingerprint_does_not_prevent_configured_cargo_build(self):
        env = development_environment()
        for name in ("TRANSMOG_BUILD_CACHE_TRIM", "CI", "CARGO_TARGET_DIR", "CARGO_BUILD_BUILD_DIR"):
            env.pop(name, None)
        with tempfile.TemporaryDirectory(prefix="transmog-cache-command-") as directory:
            root = Path(directory).resolve()
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text('[package]\nname="cache-command"\nversion="0.1.0"\nedition="2021"\n')
            (root / "src/main.rs").write_text('fn main(){println!("configured build completed");}\n')
            profile = root / "target/debug"
            (profile / "deps").mkdir(parents=True)
            malformed = profile / ".fingerprint/unrelated-0000000000000001"
            malformed.mkdir(parents=True)
            (malformed / "lib-unrelated.json").write_text("null")
            with patch("repo_tools.ROOT", root):
                result = command(["cargo", "build", "--offline"], cwd=root, env=env, capture=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((profile / ".transmog-cache-maintenance.json").exists())
            self.assertEqual((malformed / "lib-unrelated.json").read_text(), "null")
            binary = profile / ("cache-command.exe" if os.name == "nt" else "cache-command")
            self.assertEqual(subprocess.check_output([str(binary)], text=True).strip(), "configured build completed")

    def test_real_variants_rebuild_and_an_active_cargo_build_is_protected(self):
        env = development_environment()
        env["TRANSMOG_BUILD_CACHE_TRIM"] = "0"
        env["CARGO_INCREMENTAL"] = "1"
        env.pop("CARGO_TARGET_DIR", None)
        env.pop("CARGO_BUILD_BUILD_DIR", None)
        with tempfile.TemporaryDirectory(prefix="transmog-cache-cargo-") as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "helper/src").mkdir(parents=True)
            (root / "Cargo.toml").write_text('[workspace]\nmembers=["helper"]\n[package]\nname="cache-fixture"\nversion="0.1.0"\nedition="2021"\n[dependencies]\ncache-helper={path="helper"}\n')
            (root / "helper/Cargo.toml").write_text('[package]\nname="cache-helper"\nversion="0.1.0"\nedition="2021"\n')
            (root / "src/main.rs").write_text('fn main(){println!("{}",cache_helper::answer());}\n')
            (root / "helper/src/lib.rs").write_text('pub fn answer()->u32{42}\n')
            cargo = executable("cargo", env)
            def compile_variant(number):
                (root / "Cargo.toml").write_text(f'[workspace]\nmembers=["helper"]\n[package]\nname="cache-fixture"\nversion="0.1.{number}"\nedition="2021"\n[dependencies]\ncache-helper={{path="helper"}}\n')
                (root / "helper/Cargo.toml").write_text(f'[package]\nname="cache-helper"\nversion="0.1.{number}"\nedition="2021"\n')
                configured = {**env, "RUSTFLAGS": f"--cfg cache_variant_{number}"}
                result = subprocess.run([cargo, "build", "--offline", "--message-format=json"], cwd=root,
                                        env=configured, capture_output=True, text=True, timeout=120)
                self.assertEqual(result.returncode, 0, result.stderr)
                return [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
            for number in range(4):
                compile_variant(number)
            result = build_cache.maintain(root, apply=True, min_age_hours=0, min_free_gib=0)
            self.assertGreaterEqual(result["variants"], 2)
            self.assertFalse(result["errors"])
            rebuilt = compile_variant(0)
            self.assertTrue(any(item.get("reason") == "compiler-artifact" and not item["fresh"]
                                and item["target"]["name"] == "cache_helper" for item in rebuilt))
            binary = root / "target/debug" / ("cache-fixture.exe" if os.name == "nt" else "cache-fixture")
            self.assertEqual(subprocess.check_output([str(binary)], text=True).strip(), "42")
            # Cargo holds its actual profile locks while this native build script
            # waits. Cleanup must neither touch artifacts nor mark the day done.
            marker, release = root / "entered", root / "release"
            (root / "build.rs").write_text('fn main(){let marker=std::env::var("CACHE_TEST_MARKER").unwrap();std::fs::write(marker,"waiting").unwrap();let release=std::env::var("CACHE_TEST_RELEASE").unwrap();let start=std::time::Instant::now();while !std::path::Path::new(&release).exists(){assert!(start.elapsed().as_secs()<30);std::thread::sleep(std::time::Duration::from_millis(20));}}')
            active_env = {**env, "CACHE_TEST_MARKER": str(marker), "CACHE_TEST_RELEASE": str(release)}
            with subprocess.Popen([cargo, "build", "--offline"], cwd=root, env=active_env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as active:
                try:
                    deadline = time.monotonic() + 30
                    while not marker.exists() and time.monotonic() < deadline and active.poll() is None:
                        time.sleep(0.02)
                    self.assertTrue(marker.exists(), "Cargo did not reach the lock barrier")
                    report = build_cache.maintain(root, apply=True, daily=True, min_age_hours=0, min_free_gib=0)
                    self.assertEqual(report["status"], "busy")
                finally:
                    release.write_text("continue")
                    _, errors = active.communicate(timeout=30)
                self.assertEqual(active.returncode, 0, errors)


if __name__ == "__main__":
    unittest.main()
