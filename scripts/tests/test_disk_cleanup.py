"""Combined cleanup against actual Cargo layouts and owned test outputs."""
import importlib.util
import io
import json
import os
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import build_cache
from disk_cleanup import clean_disk
import test_run_cache


class DiskCleanupTests(unittest.TestCase):
    def setUp(self):
        owner = tempfile.TemporaryDirectory()
        self.addCleanup(owner.cleanup)
        self.root = Path(owner.name).resolve()

    def target(self, name):
        target = self.root / name
        target.mkdir(parents=True, exist_ok=True)
        (target / "CACHEDIR.TAG").write_text("Signature: 8a477f597d28d172789f06886806bc55\n")
        (target / ".rustc_info.json").write_text("{}")
        return target

    def variants(self, target, profile="debug"):
        owner = target / profile
        groups = []
        for number in range(1, 4):
            digest = f"{number:016x}"
            unit = owner / "build/example" / digest
            fp = unit / "fingerprint"
            fp.mkdir(parents=True)
            (unit / "out").mkdir()
            (fp / "lib-example.json").write_text(json.dumps({"rustc": 1, "target": 1, "profile": 1, "deps": []}))
            (fp / "lib-example").write_text(number.to_bytes(8, "little").hex())
            artifact = unit / "out" / f"libexample-{digest}.rlib"
            artifact.write_bytes(b"rebuildable")
            for path in [*fp.iterdir(), artifact]:
                stamp = time.time() - 86400 * 3 + number
                os.utime(path, (stamp, stamp))
            groups.append((fp, artifact))
        return groups

    def completed_run(self):
        run = self.root / "artifacts/load/completed"
        with test_run_cache.output_run(run, "load") as outcome:
            bulk = run / "cli/profile"
            bulk.mkdir(parents=True)
            (bulk / "body.cache").write_bytes(b"bulk capture")
            (run / "report.json").write_text('{"summary":"keep"}')
            (run / "stderr.txt").write_text("keep")
            (run / "failure.png").write_bytes(b"keep")
            outcome["passed"] = False
        return run, bulk

    def cli(self, *arguments):
        spec = importlib.util.spec_from_file_location("clean_disk_cli", Path(__file__).resolve().parents[1] / "clean-disk.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        module.ROOT = self.root
        output = io.StringIO()
        with patch.object(sys, "argv", ["clean-disk.py", *arguments]), redirect_stdout(output):
            module.main()
        return json.loads(output.getvalue())

    def test_default_cleanup_finds_experiments_and_preserves_evidence_and_release(self):
        target = self.target("target")
        regular = self.variants(target)
        experiment = self.target("target/debugger-qualification")
        isolated = self.variants(experiment)
        release = self.variants(target, "release")
        run, bulk = self.completed_run()
        preview = clean_disk(self.root)
        self.assertEqual(preview["testOutputs"]["runs"], 1)
        self.assertTrue(bulk.exists())
        self.assertTrue(all(path.exists() for group in regular + isolated for path in group))
        result = self.cli("--apply")
        self.assertFalse(result["errors"])
        self.assertTrue(all(not path.exists() for group in regular[:2] + isolated[:2] for path in group))
        self.assertTrue(all(path.exists() for group in regular[2:] + isolated[2:] + release for path in group))
        self.assertFalse(bulk.exists())
        self.assertTrue(all((run / name).exists() for name in ["report.json", "stderr.txt", "failure.png", test_run_cache.MANIFEST]))
        self.assertTrue(result["preservedProfiles"])
        self.assertEqual(clean_disk(self.root, apply=True)["removedLogicalBytes"], 0)

    def test_conservative_retains_variants_and_failure_bulk(self):
        groups = self.variants(self.target("target"))
        _, bulk = self.completed_run()
        result = clean_disk(self.root, apply=True, deep=False)
        self.assertFalse(result["errors"])
        self.assertEqual(result["removedLogicalBytes"], 0)
        self.assertTrue(bulk.exists())
        self.assertTrue(all(path.exists() for group in groups for path in group))

    def test_cli_defaults_to_aggressive_preview_and_supports_explicit_policies(self):
        groups = self.variants(self.target("target"))
        _, bulk = self.completed_run()
        default = self.cli()
        self.assertTrue(default["deep"])
        self.assertEqual(default["status"], "preview")
        self.assertEqual(default["testOutputs"]["runs"], 1)
        self.assertGreater(default["reclaimableBytesEstimate"], 0)
        self.assertTrue(bulk.exists())
        self.assertTrue(all(path.exists() for group in groups for path in group))
        conservative = self.cli("--conservative", "--apply")
        self.assertFalse(conservative["deep"])
        self.assertEqual(conservative["removedLogicalBytes"], 0)
        self.assertTrue(bulk.exists())
        self.assertEqual(self.cli("--deep")["reclaimableBytesEstimate"], default["reclaimableBytesEstimate"])
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as raised:
            self.cli("--deep", "--conservative")
        self.assertEqual(raised.exception.code, 2)

    def test_independent_marker_is_not_a_target_triple_and_unmarked_layout_is_preserved(self):
        target = self.target("target")
        self.variants(target)
        independent = self.target("target/debugger-perf-baseline")
        self.variants(independent)
        cross = target / "x86_64-pc-windows-msvc"
        self.variants(cross)
        unknown = target / "user-data"
        unknown_groups = self.variants(unknown)
        self.assertIn(independent, build_cache.registered_targets(self.root))
        self.assertNotIn(unknown, build_cache.registered_targets(self.root))
        self.assertEqual(build_cache.related_profiles(target, self.root), [target / "debug", cross / "debug"])
        result = clean_disk(self.root, apply=True, deep=True)
        profiles = [row["profile"] for row in result["profiles"]]
        self.assertEqual(profiles.count(str((independent / "debug").relative_to(self.root))), 1)
        self.assertTrue(all(path.exists() for group in unknown_groups for path in group))

    def test_pins_and_active_leases_survive_deep_cleanup(self):
        target = self.target("target/debugger-qualification")
        groups = self.variants(target)
        run, bulk = self.completed_run()
        (target / build_cache.PIN).touch()
        (run / build_cache.PIN).touch()
        result = clean_disk(self.root, apply=True, deep=True)
        self.assertTrue(all(path.exists() for group in groups for path in group))
        self.assertTrue(bulk.exists())
        self.assertTrue(any(row["status"] == "pinned" for row in result["profiles"]))
        (target / build_cache.PIN).unlink()
        (run / build_cache.PIN).unlink()
        with build_cache.cargo_locks(target / "debug", self.root) as acquired, \
                build_cache.file_lock(run / test_run_cache.RUN_LOCK, self.root) as run_acquired:
            self.assertTrue(acquired and run_acquired)
            result = clean_disk(self.root, apply=True, deep=True)
            self.assertTrue(any(row["status"] == "busy" for row in result["profiles"]))
            self.assertTrue(bulk.exists())
            self.assertTrue(all(path.exists() for group in groups for path in group))

    def test_release_requires_explicit_selection(self):
        target = self.target("target")
        groups = self.variants(target, "release")
        result = clean_disk(self.root, apply=True, deep=True, include_release=True)
        self.assertFalse(result["errors"])
        self.assertTrue(all(not path.exists() for group in groups[:2] for path in group))
        self.assertTrue(all(path.exists() for path in groups[2]))

    def test_containing_target_pin_protects_discovered_and_explicit_experiments(self):
        target = self.target("target/debugger-qualification")
        groups = self.variants(target)
        (self.root / "target" / build_cache.PIN).touch()
        result = clean_disk(self.root, apply=True, deep=True)
        self.assertTrue(any(row["status"] == "pinned" for row in result["profiles"]))
        self.assertEqual(build_cache.maintain(self.root, target, apply=True, keep=1,
                                            min_age_hours=0)["status"], "pinned")
        self.assertTrue(all(path.exists() for group in groups for path in group))


if __name__ == "__main__":
    unittest.main()
