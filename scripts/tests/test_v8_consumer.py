"""Reject stale/forged native consumer evidence and exercise fresh/reused checks."""
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import zipfile
from unittest.mock import patch

from test_tools import load

consumer = load("v8_consumer")

COMMIT = "a" * 40
REPOSITORY = "erik-anderson/transmog"


def zipped(report):
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w") as archive:
        archive.writestr("v8-native-consumer.json", json.dumps(report))
    return stream.getvalue()


class NativeConsumerTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.report_path = self.root / "artifacts/v8-native-consumer.json"
        self.context = {"schema": 1, "manifest": {"key": "engine", "files": {"rusty_v8.lib": "bytes"}},
                        "inputs": {"source": "hash"}, "consumerAbi": {"flags": "pinned"}}
        self.key = consumer.engine.encoded_digest(self.context)
        self.report = {"Commit": COMMIT, "RunId": "42", "RunAttempt": 1, "Target": consumer.TARGET,
                       "ValidationMode": "fresh", "CargoAbiVerified": True, "ValidationKey": self.key,
                       "Context": self.context}
        self.artifact = {"id": 123, "name": f"windows-v8-arm64-consumer-{self.key}-1", "expired": False,
                         "size_in_bytes": 1000, "workflow_run": {"id": 42, "head_sha": COMMIT, "head_branch": "main"}}
        self.payload = zipped(self.report)
        self.artifact["digest"] = "sha256:" + hashlib.sha256(self.payload).hexdigest()
        self.run = {"id": 42, "head_sha": COMMIT, "head_branch": "main", "event": "push",
                    "path": ".github/workflows/prepare-windows-v8.yml", "head_repository": {"full_name": REPOSITORY}}
        self.job = {"id": 987, "name": "engine / " + consumer.JOB, "conclusion": "success", "head_sha": COMMIT}
        environment = patch.dict(os.environ, {"GITHUB_REPOSITORY": REPOSITORY, "GITHUB_REF_NAME": "main",
            "GITHUB_SHA": "b" * 40, "GITHUB_RUN_ID": "43", "GITHUB_RUN_ATTEMPT": "2", "V8_ARTIFACT_ID": "124",
            "GITHUB_OUTPUT": "", "GITHUB_STEP_SUMMARY": ""})
        environment.start()
        self.addCleanup(environment.stop)

    def request(self, url):
        if "/actions/artifacts?" in url:
            return {"artifacts": [self.artifact]}
        if "/attempts/1/jobs?" in url:
            return {"jobs": [self.job]}
        if url.endswith("/actions/runs/42"):
            return self.run
        raise AssertionError(url)

    def test_exact_success_reuses_cargo_and_preserves_original_immutable_proof(self):
        with patch.object(consumer, "context", return_value=self.context), \
             patch.object(consumer.github, "request", side_effect=self.request), \
             patch.object(consumer, "artifact_bytes", return_value=self.payload), \
             patch.object(consumer.subprocess, "run") as cargo:
            result = consumer.verify(self.root / "engine", self.report_path, root=self.root)
        cargo.assert_not_called()
        self.assertEqual(result["ValidationMode"], "reused")
        self.assertEqual(result["ValidationOrigin"], {"ArtifactId": 123, "ArtifactDigest": self.artifact["digest"],
            "Commit": COMMIT, "RunId": "42", "RunAttempt": 1, "JobId": 987})
        self.assertEqual(result["Commit"], "b" * 40)
        self.assertEqual(result["RunId"], "43")
        self.assertEqual(result["ArtifactId"], "124")

    def test_failed_or_untrusted_proofs_cannot_skip_the_native_check(self):
        mutations = [lambda: self.job.update(conclusion="failure"), lambda: self.job.update(conclusion=None),
            lambda: self.job.update(head_sha="d" * 40), lambda: self.run.update(event="pull_request"),
            lambda: self.run.update(path=".github/workflows/untrusted.yml"),
            lambda: self.run.update(head_branch="other"), lambda: self.run.update(head_sha="d" * 40),
            lambda: self.run["head_repository"].update(full_name="fork/transmog"),
            lambda: self.report.update(CargoAbiVerified=False), lambda: self.report.update(CargoAbiVerified=1),
            lambda: self.report.update(ValidationMode="reused"), lambda: self.report.update(RunAttempt=2),
            lambda: self.report.update(Commit="d" * 40), lambda: self.report.update(RunId="999"),
            lambda: self.report.update(Context={"different": "engine or ABI"})]
        baseline = copy.deepcopy((self.job, self.run, self.report))
        for mutate in mutations:
            self.job, self.run, self.report = copy.deepcopy(baseline)
            mutate()
            with self.subTest(mutation=mutate), patch.object(consumer.github, "request", side_effect=self.request):
                with self.assertRaises(ValueError):
                    consumer.verify_origin("https://api.github.com/repos/" + REPOSITORY, self.artifact,
                                           self.report, self.context, self.key, "main")

    def test_missing_expired_or_unavailable_evidence_runs_original_cargo_command(self):
        for reason in ("missing", "expired", "unavailable", "disabled"):
            with self.subTest(reason=reason), patch.object(consumer, "context", return_value=self.context), \
                 patch.object(consumer.github, "request", side_effect=RuntimeError("HTTP unavailable") if reason == "unavailable"
                              else lambda _: {"artifacts": [] if reason == "missing" else [dict(self.artifact, expired=True)]}), \
                 patch.object(consumer.subprocess, "run") as cargo:
                result = consumer.verify(self.root / "engine", self.report_path, use_cache=reason != "disabled", root=self.root)
                self.assertEqual(cargo.call_args.args[0], ["cargo", "check", "--locked", "-p", "transmog-js-runtime",
                                                          "--target", "aarch64-pc-windows-msvc"])
                self.assertEqual(cargo.call_args.kwargs["env"]["TRANSMOG_V8_ARTIFACT"], str(self.root / "engine"))
                self.assertTrue(cargo.call_args.kwargs["check"])
                self.assertEqual(result["ValidationMode"], "fresh")

    def test_failed_fresh_check_does_not_leave_a_success_report(self):
        self.report_path.parent.mkdir()
        self.report_path.write_text("stale success")
        with patch.object(consumer, "context", return_value=self.context), \
             patch.object(consumer.subprocess, "run", side_effect=subprocess.CalledProcessError(1, consumer.COMMAND)):
            with self.assertRaises(subprocess.CalledProcessError):
                consumer.verify(self.root / "engine", self.report_path, use_cache=False, root=self.root)
        self.assertFalse(self.report_path.exists())

    def test_artifact_digest_and_ambiguous_report_rejected(self):
        class Response(io.BytesIO):
            pass
        with patch.object(consumer.github, "urlopen", return_value=Response(self.payload)):
            self.assertEqual(consumer.artifact_bytes("https://api.github.com/repos/" + REPOSITORY, self.artifact), self.payload)
        with patch.object(consumer.github, "urlopen", return_value=Response(self.payload + b"tampered")):
            with self.assertRaisesRegex(ValueError, "digest"):
                consumer.artifact_bytes("https://api.github.com/repos/" + REPOSITORY, self.artifact)
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, "w") as archive:
            archive.writestr("one/v8-native-consumer.json", "{}")
            archive.writestr("two/v8-native-consumer.json", "{}")
        with self.assertRaisesRegex(ValueError, "ambiguous"):
            consumer.read_report(stream.getvalue())

    def test_native_host_and_fresh_engine_verification_are_required_even_with_evidence(self):
        with patch.object(consumer.engine, "raw_tools", return_value={"rustc": "host: x86_64-pc-windows-msvc"}):
            with self.assertRaisesRegex(ValueError, "native ARM64"):
                consumer.context(self.root, self.root)
        with patch.object(consumer.engine, "raw_tools", return_value={"rustc": "host: " + consumer.TARGET}), \
             patch.object(consumer.engine, "validate", side_effect=ValueError("engine SDK or payload changed")):
            with self.assertRaisesRegex(ValueError, "engine SDK or payload"):
                consumer.context(self.root, self.root)

    def test_consumer_dependency_source_and_flags_invalidate_reuse(self):
        # A real tracked local dependency graph, including a dependency outside the consumer crate.
        for name in (*consumer.INPUTS, "crates/js-runtime/src/lib.rs", "crates/shared/src/lib.rs", "unrelated.txt",
                     "vendor/dep/Cargo.toml", "vendor/dep/src/lib.rs"):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)
        (self.root / "Cargo.toml").write_text('[patch.crates-io]\npatched = { path = "vendor/dep" }\n')
        (self.root / "Cargo.lock").write_text('[[package]]\nname="transmog-js-runtime"\nversion="1.0.0"\n'
            'dependencies=["patched"]\n[[package]]\nname="patched"\nversion="2.0.0"\n')
        (self.root / "vendor/dep/Cargo.toml").write_text('[package]\nname="patched"\nversion="2.0.0"\n')
        subprocess.run(["git", "init", "-q"], cwd=self.root, check=True)
        subprocess.run(["git", "add", "."], cwd=self.root, check=True)
        metadata = {"packages": [{"name": "transmog-js-runtime", "manifest_path": str(self.root / "crates/js-runtime/Cargo.toml"),
                                 "dependencies": [{"path": str(self.root / "crates/shared")}]},
                                {"name": "shared", "manifest_path": str(self.root / "crates/shared/Cargo.toml"), "dependencies": []}]}
        original = subprocess.check_output
        def output(arguments, **kwargs):
            return json.dumps(metadata) if arguments[0] == "cargo" else original(arguments, **kwargs)
        with patch.object(consumer.subprocess, "check_output", side_effect=output), \
             patch.object(consumer.engine, "raw_tools", return_value={"rustc": "host: " + consumer.TARGET}), \
             patch.object(consumer.engine, "validate", return_value=self.context["manifest"]), \
             patch.object(consumer.engine, "abi", return_value={"flags": "pinned"}) as abi:
            baseline = consumer.engine.encoded_digest(consumer.context(self.root, self.root))
            (self.root / "unrelated.txt").write_text("unrelated change")
            self.assertEqual(consumer.engine.encoded_digest(consumer.context(self.root, self.root)), baseline)
            for name in ("crates/shared/src/lib.rs", "vendor/dep/src/lib.rs", "scripts/v8_consumer.py"):
                path = self.root / name
                old = path.read_text()
                path.write_text("changed consumer input")
                self.assertNotEqual(consumer.engine.encoded_digest(consumer.context(self.root, self.root)), baseline)
                path.write_text(old)
            lock = self.root / "Cargo.lock"
            original_lock = lock.read_text()
            lock.write_text(original_lock + '\n[[package]]\nname="new-dependency"\nversion="1.0.0"\n')
            self.assertNotEqual(consumer.engine.encoded_digest(consumer.context(self.root, self.root)), baseline)
            lock.write_text(original_lock)
            abi.return_value = {"flags": "different target feature"}
            self.assertNotEqual(consumer.engine.encoded_digest(consumer.context(self.root, self.root)), baseline)


if __name__ == "__main__":
    unittest.main()
