"""Cache reuse, concurrent writers, and unavailable checkpoints stay distinct."""
import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from test_tools import load

checkpoint = load("cache-checkpoint")


class CacheCheckpointTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def report(self, **overrides):
        arguments = dict(enabled=True, restored=False, existing=False, available=True,
                         report_path=self.root / "report.json", summary_path=self.root / "summary.md",
                         output_path=self.root / "outputs")
        arguments.update(overrides)
        with contextlib.redirect_stdout(io.StringIO()):
            return checkpoint.report("V8 x64", "exact-key", **arguments)

    def test_restored_cache_is_reused_without_claiming_a_new_save(self):
        result = self.report(restored=True)
        self.assertEqual(result["status"], "reused")
        self.assertIn("upload skipped", result["message"])

    def test_cache_published_while_compiling_is_an_expected_reuse(self):
        result = self.report(existing=True)
        self.assertEqual(result["status"], "reused")
        self.assertIn("another producer", result["message"])
        self.assertIn("duplicate upload skipped", result["message"])

    def test_availability_after_upload_does_not_guess_which_writer_won(self):
        result = self.report()
        self.assertEqual(result["status"], "available")
        self.assertNotIn("saved by this run", result["message"])
        self.assertEqual(json.loads((self.root / "report.json").read_text())["key"], "exact-key")
        self.assertEqual((self.root / "outputs").read_text(), "status=available\n")

    def test_missing_cache_fails_and_retains_honest_summary_and_evidence(self):
        for restored, existing in ((False, False), (True, False), (False, True)):
            with self.subTest(restored=restored, existing=existing):
                with self.assertRaisesRegex(RuntimeError, "checkpoint failed"):
                    self.report(available=False, restored=restored, existing=existing)
                self.assertEqual(json.loads((self.root / "report.json").read_text())["status"], "unavailable")
                self.assertIn("cache: unavailable", (self.root / "summary.md").read_text())

    def test_explicit_no_cache_run_does_not_require_a_checkpoint(self):
        result = self.report(enabled=False, available=False)
        self.assertEqual(result["status"], "disabled")
        self.assertIn("no lookup or upload", result["message"])

    def test_invalid_cache_flag_cannot_silently_disable_checkpoint(self):
        with patch.dict(os.environ, CHECKPOINT_ENABLED="maybe"):
            with self.assertRaisesRegex(ValueError, "CHECKPOINT_ENABLED must be"):
                checkpoint.flag("CHECKPOINT_ENABLED")


if __name__ == "__main__":
    unittest.main()
