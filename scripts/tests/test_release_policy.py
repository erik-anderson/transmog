"""Release checks preserve failures and use a source-bound advisory database."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from test_tools import load

policy = load("release-policy")


class ReleasePolicyTests(unittest.TestCase):
    def test_snapshot_rejects_different_source_locks_revision_and_dirty_database(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            repository = root / "db/advisory"
            repository.mkdir(parents=True)
            subprocess.run(["git", "init", "-q", str(repository)], check=True)
            entry = repository / "entry.md"
            entry.write_text("advisory", encoding="utf-8")
            subprocess.run(["git", "-C", str(repository), "add", "entry.md"], check=True)
            subprocess.run(["git", "-C", str(repository), "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "fixture"], check=True)
            record = {"schema": 1, "sourceCommit": "source", "inputs": {"Cargo.lock": "hash"},
                      "databaseCommit": policy.git(repository, "rev-parse", "HEAD"), "databasePath": "db/advisory"}
            (root / "snapshot.json").write_text(json.dumps(record), encoding="utf-8")
            self.assertEqual(policy.validate_snapshot(root, "source", record["inputs"]), record)
            for source, hashes in (("other", record["inputs"]), ("source", {"Cargo.lock": "changed"})):
                with self.assertRaisesRegex(ValueError, "does not match"):
                    policy.validate_snapshot(root, source, hashes)
            entry.write_text("modified advisory", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "must be clean"):
                policy.validate_snapshot(root, "source", record["inputs"])
            subprocess.run(["git", "-C", str(repository), "checkout", "--", "entry.md"], check=True)
            record["databaseCommit"] = "0" * 40
            (root / "snapshot.json").write_text(json.dumps(record), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "revision differs"):
                policy.validate_snapshot(root, "source", record["inputs"])

    def test_snapshot_database_path_cannot_escape_its_root(self):
        for name in ("../outside", "db/../outside", "C:/outside", "db\\outside", "/outside"):
            with self.subTest(name=name), self.assertRaises(ValueError):
                policy.snapshot_repository(Path("snapshot"), {"databasePath": name})

    def test_diagnostics_summarize_warnings_without_losing_errors(self):
        text = "\n".join(json.dumps({"type": "diagnostic", "fields": fields}) for fields in (
            {"severity": "warning", "code": "duplicate", "message": "dependency tree"},
            {"severity": "warning", "code": "duplicate", "message": "another tree"},
            {"severity": "error", "code": "vulnerability", "message": "affected crate", "notes": ["RUSTSEC-fixture"]},
        ))
        counts, errors = policy.diagnostics(text)
        self.assertEqual(counts, {"warning:duplicate": 2, "error:vulnerability": 1})
        self.assertEqual(errors[0]["notes"], ["RUSTSEC-fixture"])

    def test_nonzero_check_retains_failure_report_and_checks_both_graphs_offline(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in policy.INPUTS:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("[advisories]\n" if name.endswith("deny.toml") else "fixture", encoding="utf-8")
            metadata = {"databaseCommit": "a" * 40, "capturedAtUtc": "fixture"}
            failed = json.dumps({"type": "diagnostic", "fields": {"severity": "error", "code": "vulnerability", "message": "fixture vulnerability"}})
            def run(arguments, **kwargs):
                if "check" in arguments:
                    self.assertIn("--locked", arguments)
                    self.assertIn("--offline", arguments)
                    return subprocess.CompletedProcess(arguments, 1, "", failed)
                return subprocess.CompletedProcess(arguments, 0)
            argv = ["release-policy", "--snapshot", str(root / "snapshot"), "--output", str(root / "output")]
            with patch.object(sys, "argv", argv), patch.object(policy, "ROOT", root), patch.object(policy, "git", return_value="source"), patch.object(policy, "validate_snapshot", return_value=metadata), patch.object(policy.subprocess, "run", side_effect=run):
                with self.assertRaisesRegex(ValueError, "dependency policy failed"):
                    policy.main()
            report = json.loads((root / "output/policy.json").read_text(encoding="utf-8"))
            self.assertEqual(report["result"], "failed")
            self.assertEqual([check["graph"] for check in report["checks"]], ["workspace", "fuzz"])
            self.assertIn("fixture vulnerability", (root / "output/workspace.jsonl").read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
