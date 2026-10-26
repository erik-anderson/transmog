"""Completed-run ownership, active leases, evidence retention and budgets."""
import json
import os
from pathlib import Path
import subprocess
import shutil
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import build_cache
import test_run_cache as cache


class OutputCacheTests(unittest.TestCase):
    def setUp(self):
        self.owner = tempfile.TemporaryDirectory()
        self.addCleanup(self.owner.cleanup)
        self.root = Path(self.owner.name).resolve()
        self.now = time.time()

    def run_output(self, name, *, passed=True, age=20, kind="load"):
        run = self.root / cache.ROOTS[kind] / name
        with cache.output_run(run, kind) as outcome:
            profile = run / cache.BULK[kind][0]
            if profile.suffix:
                profile.parent.mkdir(parents=True)
                profile.write_bytes(b"bulk capture")
            else:
                profile.mkdir(parents=True)
                (profile / "body.cache").write_bytes(b"bulk profile")
            (run / "report.json").write_text('{"summary":"retained"}')
            (run / "stderr.txt").write_text("diagnostic log")
            (run / "failure.png").write_bytes(b"screenshot")
            outcome["passed"] = passed
        manifest = json.loads((run / cache.MANIFEST).read_text())
        manifest["finishedAt"] = self.now - age * 86400
        (run / cache.MANIFEST).write_text(json.dumps(manifest))
        return run, profile

    def maintain(self, **kwargs):
        return cache.maintain_outputs(self.root, now=self.now, **kwargs)

    def test_expiration_preserves_summaries_failures_pins_and_newest_successes(self):
        runs = [self.run_output(str(i), age=30-i) for i in range(5)]
        failure, failure_bulk = self.run_output("failure", passed=False)
        pinned, pinned_bulk = self.run_output("pinned")
        (pinned / build_cache.PIN).touch()
        unknown = self.root / cache.ROOTS["load"] / "unregistered"
        unknown.mkdir()
        (unknown / "user-data").write_bytes(b"keep")
        preview = self.maintain()
        self.assertEqual(preview["runs"], 3)
        self.assertTrue(all(p.exists() for _, p in runs))
        result = self.maintain(apply=True)
        self.assertEqual(result["runs"], 3)
        self.assertFalse(result["errors"])
        self.assertTrue(all(not p.exists() for _, p in runs[:3]))
        self.assertTrue(all(p.exists() for _, p in runs[3:]))
        self.assertTrue(failure_bulk.exists() and pinned_bulk.exists())
        self.assertTrue((unknown / "user-data").exists())
        for run, _ in runs:
            self.assertTrue(all((run / name).exists() for name in ("report.json", "stderr.txt", "failure.png")))
        self.assertEqual(self.maintain(apply=True)["runs"], 0)

    def test_budget_prunes_oldest_excess_runs_even_before_expiration(self):
        runs = [self.run_output(str(i), age=1-i/10) for i in range(4)]
        result = self.maintain(apply=True, max_gib=0, max_age_days=100, keep_success=1)
        self.assertEqual(result["runs"], 3)
        self.assertTrue(runs[-1][1].exists())

    def test_pinned_bulk_counts_toward_the_budget(self):
        pin, pin_bulk = self.run_output("pin", age=3)
        old, old_bulk = self.run_output("old", age=2)
        new, new_bulk = self.run_output("new", age=1)
        (pin / build_cache.PIN).touch()
        result = self.maintain(apply=True, max_gib=30 / 1024**3,
                              max_age_days=100, keep_success=1)
        self.assertEqual(result["runs"], 1)
        self.assertFalse(old_bulk.exists())
        self.assertTrue(new_bulk.exists() and pin_bulk.exists())

    def test_failed_staging_preserves_bulk_data_and_reports(self):
        run, bulk = self.run_output("locked")
        rename = Path.rename
        def refuse(path, destination):
            if path == bulk:
                raise PermissionError("fixture loaded capture")
            return rename(path, destination)
        with patch.object(Path, "rename", refuse):
            result = self.maintain(apply=True, keep_success=0)
        self.assertEqual(len(result["errors"]), 1)
        self.assertTrue(bulk.exists())
        self.assertTrue((run / "report.json").exists())
        self.assertNotIn("bulkPrunedAt", json.loads((run / cache.MANIFEST).read_text()))

    def test_interrupted_staging_is_recovered_under_the_run_lease(self):
        run, bulk = self.run_output("staged")
        trash = build_cache.checked(run / ".transmog-cache-trash" / ("a" * 32), self.root)
        trash.mkdir(parents=True)
        bulk.rename(trash / bulk.name)
        result = self.maintain(apply=True)
        self.assertGreater(result["recoveredTrashLogicalBytes"], 0)
        self.assertFalse(trash.exists())
        self.assertTrue((run / "report.json").exists())

    def test_running_interrupted_and_invalid_metadata_remain_protected(self):
        run = self.root / cache.ROOTS["desktop"] / "live"
        with cache.output_run(run, "desktop"):
            (run / "profile").mkdir()
            (run / "profile/file").write_bytes(b"live")
            result = self.maintain(apply=True, keep_success=0, keep_failures=0, max_gib=0)
            self.assertEqual(result["runs"], 0)
            self.assertTrue(any(row["reason"] == "active or busy" for row in result["retained"]))
        self.assertEqual(json.loads((run / cache.MANIFEST).read_text())["state"], "interrupted")
        self.assertEqual(self.maintain(apply=True, max_gib=0)["runs"], 0)
        (run / cache.MANIFEST).write_text("null")
        self.assertEqual(self.maintain(apply=True, max_gib=0)["runs"], 0)
        self.assertTrue((run / "profile/file").exists())

    def test_manifest_cannot_nominate_arbitrary_paths(self):
        run, bulk = self.run_output("one")
        valuable = self.root / "keep.txt"
        valuable.write_bytes(b"keep")
        manifest = json.loads((run / cache.MANIFEST).read_text())
        manifest["bulkPaths"] = [str(valuable), "../../../../keep.txt"]
        (run / cache.MANIFEST).write_text(json.dumps(manifest))
        self.maintain(apply=True, keep_success=0)
        self.assertTrue(valuable.exists())
        self.assertFalse(bulk.exists())

    def test_legacy_adoption_is_explicit_and_requires_completed_reports(self):
        run, bulk = self.run_output("legacy")
        (run / cache.MANIFEST).unlink()
        report = {"schema": 1, "passed": True, "finishedAt": "2020-01-01T00:00:00Z",
                  "results": [{"passed": True}], "config": {"target": "cli"}}
        (run / "report.json").write_text(json.dumps(report))
        self.assertEqual(self.maintain(keep_success=0)["runs"], 0)
        report["results"][0]["cleanupError"] = "fixture shutdown did not finish"
        (run / "report.json").write_text(json.dumps(report))
        self.assertEqual(self.maintain(include_legacy=True, keep_success=0)["runs"], 0)
        del report["results"][0]["cleanupError"]
        (run / "report.json").write_text(json.dumps(report))
        self.assertEqual(self.maintain(apply=True, include_legacy=True, keep_success=0)["runs"], 1)
        self.assertFalse(bulk.exists())
        self.assertTrue((run / "report.json").exists())
        self.assertTrue(json.loads((run / cache.MANIFEST).read_text())["legacy"])

    def test_subprocess_lease_blocks_cleanup_until_explicit_completion(self):
        run = self.root / cache.ROOTS["desktop"] / "subprocess"
        script = Path(__file__).resolve().parents[1] / "test-output-run.py"
        with subprocess.Popen([sys.executable, script, "--path", run, "--kind", "desktop"],
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              text=True) as child:
            try:
                self.assertTrue(json.loads(child.stdout.readline())["ready"])
                (run / "profile").mkdir()
                (run / "profile/data").write_bytes(b"active")
                self.assertEqual(self.maintain(apply=True, max_gib=0, keep_success=0)["runs"], 0)
            finally:
                _, stderr = child.communicate('{"passed":true}\n', timeout=15)
            self.assertEqual(child.returncode, 0, stderr)
        result = cache.maintain_outputs(self.root, apply=True, max_gib=0, keep_success=0)
        self.assertEqual(result["runs"], 1)
        self.assertFalse((run / "profile").exists())

    @unittest.skipUnless(os.name == "nt", "Windows native probe bridge")
    def test_powershell_bridge_publishes_completion(self):
        script = self.root / "probe.ps1"
        script.write_text('''param([string]$Helper, [string]$Run)
$ErrorActionPreference='Stop'
. $Helper
$lease=Start-TestOutputRun $Run 'fixture'
try {
 if ((Get-Content -LiteralPath (Join-Path $Run '.transmog-test-run.json') -Raw | ConvertFrom-Json).state -ne 'running') { throw 'Lease did not start' }
} finally { Complete-TestOutputRun $lease $false $true }
''')
        run = self.root / cache.ROOTS["desktop"] / "bridge"
        helper = Path(__file__).resolve().parents[1] / "test-output-run.ps1"
        env = dict(os.environ, TRANSMOG_PYTHON=sys.executable)
        result = subprocess.run([shutil.which("pwsh"), "-NoProfile", "-File", script, helper, run],
                                env=env, capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        manifest = json.loads((run / cache.MANIFEST).read_text())
        self.assertEqual(manifest["state"], "completed")
        self.assertFalse(manifest["passed"])


if __name__ == "__main__":
    unittest.main()
