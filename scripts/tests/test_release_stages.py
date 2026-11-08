"""Exercise engine reuse and release evidence aggregation without rebuilding V8."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from test_tools import load
import v8_artifact as engine
import release_stages as stages

TARGET = engine.TARGETS[0]
COMMIT = "a" * 40


def coff_library(machine=0x8664):
    import struct
    data = struct.pack("<HHIIIHH", machine, 0, 0, 0, 0, 0, 0)
    header = (b"object.obj/".ljust(16) + b"0".ljust(12) + b"0".ljust(6) + b"0".ljust(6)
              + b"0".ljust(8) + str(len(data)).encode().ljust(10) + b"`\n")
    return b"!<arch>\n" + header + data


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


class EngineArtifactTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        for name in (*engine.INPUTS, "third_party/rusty-v8/profile.gn"):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)
        (self.root / "Cargo.toml").write_text(
            '[workspace.package]\nversion = "0.1.20"\n[workspace.dependencies]\n'
            'v8 = { version = "=152.2.0", features = ["simdutf"] }\n'
            '[patch.crates-io]\nv8 = { path = "third_party/rusty-v8" }\n')
        runtime = self.root / "crates/js-runtime/Cargo.toml"
        runtime.parent.mkdir(parents=True, exist_ok=True)
        runtime.write_text('[dependencies]\nv8 = { workspace = true }\napp = { version = "0.1.20" }\n')
        (self.root / "Cargo.lock").write_text(
            '[[package]]\nname = "v8"\nversion = "152.2.0"\ndependencies = ["bindgen"]\n'
            '[[package]]\nname = "bindgen"\nversion = "0.72.0"\n'
            '[[package]]\nname = "unrelated"\nversion = "1.0.0"\n')
        self.directory = self.root / "export"
        self.tools = patch.object(engine, "tools", return_value={"rustc": "pinned", "clang": "pinned"})
        self.tools.start()
        self.addCleanup(self.tools.stop)
        self.raw_tools = patch.object(engine, "raw_tools", return_value={"rustc": "pinned\nhost: " + TARGET, "clang": "pinned"})
        self.raw_tools.start()
        self.addCleanup(self.raw_tools.stop)
        self.environment = patch.dict(os.environ, {name: "" for name in engine.ABI_ENV})
        self.environment.start()
        self.addCleanup(self.environment.stop)
        with patch.object(engine.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, COMMIT)):
            source = self.root / "native-output"
            source.write_bytes(coff_library())
            engine.export(self.directory, TARGET, source, source, source, self.root)

    def test_engine_survives_unrelated_application_and_lockfile_changes(self):
        cargo = self.root / "Cargo.toml"
        cargo.write_text(cargo.read_text().replace('version = "0.1.20"', 'version = "0.1.21"'))
        runtime = self.root / "crates/js-runtime/Cargo.toml"
        runtime.write_text(runtime.read_text().replace('version = "0.1.20"', 'version = "0.1.21"'))
        lockfile = self.root / "Cargo.lock"
        lockfile.write_text(lockfile.read_text().replace('version = "1.0.0"', 'version = "1.0.1"'))
        (self.root / "app.rs").write_text("new application source")
        self.assertEqual(engine.validate(self.directory, TARGET, self.root, cargo=True)["originCommit"], COMMIT)

    def test_engine_rejects_hash_profile_target_tools_and_abi_changes(self):
        actions = (
            lambda: (self.directory / "rusty_v8.lib").write_bytes(b"tampered"),
            lambda: (self.root / "third_party/rusty-v8/profile.gn").write_text("changed GN profile"),
            lambda: os.environ.__setitem__("CARGO_CFG_TARGET_FEATURE", "avx2"),
        )
        for index, action in enumerate(actions):
            with self.subTest(index=index):
                path = self.directory / "rusty_v8.lib" if index == 0 else self.root / "third_party/rusty-v8/profile.gn"
                original = path.read_bytes()
                action()
                with self.assertRaises(ValueError):
                    engine.validate(self.directory, TARGET, self.root, cargo=True)
                path.write_bytes(original)
                os.environ["CARGO_CFG_TARGET_FEATURE"] = ""
        with self.assertRaisesRegex(ValueError, "target or compiler"):
            engine.validate(self.directory, engine.TARGETS[1], self.root)
        with patch.object(engine, "tools", return_value={"clang": "new compiler"}):
            with self.assertRaisesRegex(ValueError, "target or compiler"):
                engine.validate(self.directory, TARGET, self.root)

    def test_engine_rejects_changed_workspace_v8_selection(self):
        cargo = self.root / "Cargo.toml"
        cargo.write_text(cargo.read_text().replace('["simdutf"]', '["simdutf", "v8_enable_sandbox"]'))
        with self.assertRaisesRegex(ValueError, "source, profile"):
            engine.validate(self.directory, TARGET, self.root)

    def test_cross_producer_is_reused_with_native_arm64_defaults_and_retained_host_provenance(self):
        source = self.root / "arm64.lib"
        source.write_bytes(coff_library(0xAA64))
        directory = self.root / "arm64-engine"
        with patch.object(engine.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, COMMIT)):
            engine.export(directory, engine.TARGETS[1], source, source, source, self.root)
        with patch.dict(os.environ, {"CFLAGS": "--target=" + engine.TARGETS[1], "CXXFLAGS": "--target=" + engine.TARGETS[1]}):
            manifest = engine.validate(directory, engine.TARGETS[1], self.root, cargo=True)
        self.assertEqual(manifest["producerHost"], TARGET)
        manifest["producerHost"] = engine.TARGETS[1]
        write(directory / "manifest.json", manifest)
        with self.assertRaisesRegex(ValueError, "producer provenance"):
            engine.validate(directory, engine.TARGETS[1], self.root)

    def test_engine_rejects_changed_locked_bindgen_dependency(self):
        lockfile = self.root / "Cargo.lock"
        lockfile.write_text(lockfile.read_text().replace('version = "0.72.0"', 'version = "0.72.1"'))
        with self.assertRaisesRegex(ValueError, "source, profile"):
            engine.validate(self.directory, TARGET, self.root)

    def test_engine_requires_complete_payload_and_producer(self):
        manifest = json.loads((self.directory / "manifest.json").read_text())
        manifest["originRunId"] = ""
        write(self.directory / "manifest.json", manifest)
        with self.assertRaisesRegex(ValueError, "producer"):
            engine.validate(self.directory, TARGET, self.root)
        (self.directory / "unexpected.exe").touch()
        with self.assertRaisesRegex(ValueError, "unexpected"):
            engine.validate(self.directory, TARGET, self.root)

    def test_cold_export_requests_verbose_build_and_rejects_no_export(self):
        module = load("cache-v8")
        with patch.object(module, "development_environment", return_value={}), patch.object(module, "command") as command:
            with patch("sys.argv", ["cache-v8", "--export", str(self.root / "absent"), "--verbose", "--target", TARGET]):
                with self.assertRaisesRegex(RuntimeError, "did not export"):
                    module.main()
            self.assertEqual(command.call_args.kwargs["env"]["TRANSMOG_V8_EXPORT"], str((self.root / "absent").resolve()))
            self.assertIn("-vv", command.call_args.args[0])


class ReleaseStageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.sources = {stage: self.root / stage for stage in stages.GATES}
        self.ids = dict(zip(stages.GATES, ("101", "102", "103")))
        self.output = self.root / "merged"
        self.inputs = patch.object(stages, "input_hashes", return_value={"source": "reviewed"})
        self.inputs.start()
        self.addCleanup(self.inputs.stop)
        native_identity = {"inputs": {"source": "reviewed"}, "target": TARGET}
        self.engine = {"Commit": COMMIT, "RunId": "123", "Target": TARGET, "ArtifactId": "100",
                       "ManifestSha256": "e" * 64,
                       "Engine": {"identity": native_identity, "key": engine.encoded_digest(native_identity)}}
        for stage, root in self.sources.items():
            for gate in stages.GATES[stage]:
                write(root / f"evidence/gates/{gate}.json", {
                    "Schema": stages.SCHEMA, "Name": gate, "Result": "passed", "Stage": stage, "Commit": COMMIT,
                    "RunId": "123", **stages.stage_identity(stage, TARGET), "RunAttempt": "1",
                })
            if stage != "Frontend":
                write(root / f"evidence/v8-engine-{stage.lower()}.json", self.engine)
            stages.export(root, stage, COMMIT, "123", TARGET, "1")
        write(self.sources["Package"] / "evidence/package-metadata.json", {
            "Commit": COMMIT, "RunId": "123", "Target": TARGET, "Version": "0.1.20",
            "Channel": "Canary", "ReleaseType": "Canary", "SourceBranch": "main",
        })
        self.refresh("Package")

    def refresh(self, stage):
        root = self.sources[stage]
        manifest = json.loads((root / "stage-manifest.json").read_text())
        manifest["Files"] = stages.catalog(root)
        write(root / "stage-manifest.json", manifest)

    def merge(self, **kwargs):
        stages.merge(kwargs.get("sources", self.sources), kwargs.get("ids", self.ids),
                     self.output, COMMIT, "123", kwargs.get("target", TARGET), kwargs.get("attempt", "1"))

    def test_merges_complete_gates_and_retains_immutable_producer_ids(self):
        self.merge(attempt="2")
        manifest = json.loads((self.output / "build-manifest.json").read_text())
        self.assertEqual(manifest["PipelineSchema"], 2)
        self.assertEqual(manifest["ReleaseType"], "Canary")
        provenance = json.loads((self.output / "evidence/pipeline-inputs.json").read_text())
        self.assertEqual(provenance["Stages"]["Validation"]["ArtifactId"], "102")
        self.assertEqual(provenance["Stages"]["Validation"]["RunAttempt"], "1")
        self.assertEqual(provenance["Stages"]["Frontend"]["Target"], "shared")
        self.assertEqual(provenance["Stages"]["Frontend"]["HostTarget"], TARGET)
        self.assertEqual(len(list((self.output / "evidence/gates").glob("*.json"))), 8)

    def test_both_native_targets_consume_the_same_unchanged_shared_frontend(self):
        self.merge()
        first = json.loads((self.output / "evidence/pipeline-inputs.json").read_text())
        frontend_digest = engine.digest(self.sources["Frontend"] / "stage-manifest.json")
        arm = engine.TARGETS[1]
        native = json.loads(json.dumps(self.engine))
        native["Target"] = arm
        native["ArtifactId"] = "200"
        native["Engine"]["identity"]["target"] = arm
        native["Engine"]["key"] = engine.encoded_digest(native["Engine"]["identity"])
        for stage in ("Validation", "Package"):
            root = self.sources[stage]
            for gate in stages.GATES[stage]:
                path = root / f"evidence/gates/{gate}.json"
                value = json.loads(path.read_text())
                write(path, dict(value, Target=arm))
            write(root / f"evidence/v8-engine-{stage.lower()}.json", native)
            if stage == "Package":
                path = root / "evidence/package-metadata.json"
                write(path, dict(json.loads(path.read_text()), Target=arm))
            stages.export(root, stage, COMMIT, "123", arm, "1")
        self.output = self.root / "arm64-merged"
        self.merge(target=arm, ids={"Frontend": "101", "Validation": "202", "Package": "203"})
        second = json.loads((self.output / "evidence/pipeline-inputs.json").read_text())
        self.assertEqual(first["Stages"]["Frontend"], second["Stages"]["Frontend"])
        self.assertEqual(engine.digest(self.sources["Frontend"] / "stage-manifest.json"), frontend_digest)
        for name in ("UI", "Browser"):
            gate = json.loads((self.output / f"evidence/gates/{name}.json").read_text())
            self.assertEqual((gate["Target"], gate["HostTarget"]), ("shared", TARGET))
        self.assertEqual(second["Stages"]["Validation"]["Target"], arm)

    def test_rejects_mislabelled_or_legacy_frontend_provenance(self):
        root = self.sources["Frontend"]
        path = root / "stage-manifest.json"
        original = json.loads(path.read_text())
        for changes in ({"Target": TARGET}, {"Target": engine.TARGETS[1]},
                        {"HostTarget": engine.TARGETS[1]}, {"HostTarget": None}, {"Schema": 1}):
            with self.subTest(changes=changes):
                write(path, dict(original, **changes))
                with self.assertRaisesRegex(ValueError, "source, run, attempt, target or producer"):
                    self.merge()
        write(path, original)
        for gate in stages.GATES["Frontend"]:
            path = root / f"evidence/gates/{gate}.json"
            value = json.loads(path.read_text())
            for changes in ({"Target": TARGET}, {"HostTarget": engine.TARGETS[1]}, {"Schema": 1}):
                with self.subTest(gate=gate, changes=changes):
                    write(path, dict(value, **changes))
                    self.refresh("Frontend")
                    with self.assertRaisesRegex(ValueError, "did not pass"):
                        self.merge()
            write(path, value)
            self.refresh("Frontend")

    def test_shared_frontend_cannot_relabel_native_validation(self):
        path = self.sources["Validation"] / "evidence/gates/Native.json"
        write(path, dict(json.loads(path.read_text()), Target="shared", HostTarget=TARGET))
        self.refresh("Validation")
        with self.assertRaisesRegex(ValueError, "did not pass"):
            self.merge()

    def test_frontend_export_requires_x64_execution_target(self):
        with self.assertRaisesRegex(ValueError, "execute on Windows x64"):
            stages.export(self.sources["Frontend"], "Frontend", COMMIT, "123", engine.TARGETS[1], "1")

    def test_rejects_missing_null_or_incompatible_package_release_type(self):
        path = self.sources["Package"] / "evidence/package-metadata.json"
        metadata = json.loads(path.read_text())
        for index, track in enumerate((None, "", "Unknown", "Stable", "missing")):
            with self.subTest(track=track):
                invalid = dict(metadata, ReleaseType=track)
                if track == "missing":
                    del invalid["ReleaseType"]
                write(path, invalid)
                self.refresh("Package")
                self.output = self.root / f"invalid-track-{index}"
                with self.assertRaises(ValueError):
                    self.merge()

    def test_rejects_failed_gate_even_if_catalog_was_updated(self):
        path = self.sources["Validation"] / "evidence/gates/Tests.json"
        gate = json.loads(path.read_text())
        gate["Result"] = "failed"
        write(path, gate)
        self.refresh("Validation")
        with self.assertRaisesRegex(ValueError, "did not pass"):
            self.merge()

    def test_rejects_missing_gate_and_missing_producer(self):
        (self.sources["Frontend"] / "evidence/gates/Browser.json").unlink()
        self.refresh("Frontend")
        with self.assertRaisesRegex(ValueError, "gate set"):
            self.merge()
        with self.assertRaisesRegex(ValueError, "Every required"):
            self.merge(sources={"Package": self.sources["Package"]})

    def test_rejects_old_source_run_target_future_attempt_and_tampered_bytes(self):
        root = self.sources["Validation"]
        path = root / "stage-manifest.json"
        original = json.loads(path.read_text())
        for name, value in (("Commit", "b" * 40), ("RunId", "124"), ("Target", engine.TARGETS[1]), ("RunAttempt", "3")):
            with self.subTest(name=name):
                write(path, dict(original, **{name: value}))
                with self.assertRaises(ValueError):
                    self.merge()
        write(path, original)
        (root / "evidence/gates/Tests.json").write_text("changed bytes")
        with self.assertRaisesRegex(ValueError, "catalog"):
            self.merge()

    def test_frontend_cannot_supply_packaged_executables(self):
        write(self.sources["Frontend"] / "target/release/transmog.exe", {})
        self.refresh("Frontend")
        with self.assertRaisesRegex(ValueError, "Unexpected Frontend producer path"):
            self.merge()

    def test_rejects_different_engine_between_validation_and_packaging(self):
        other = dict(self.engine, ArtifactId="99")
        write(self.sources["Package"] / "evidence/v8-engine-package.json", other)
        self.refresh("Package")
        with self.assertRaisesRegex(ValueError, "different V8"):
            self.merge()

    def test_rejects_engine_from_other_source_profile_and_duplicate_artifact_ids(self):
        with patch.object(stages, "input_hashes", return_value={"source": "new profile"}):
            with self.assertRaisesRegex(ValueError, "inputs differ"):
                self.merge()
        with self.assertRaisesRegex(ValueError, "distinct"):
            self.merge(ids={stage: "100" for stage in stages.GATES})

    def test_rejects_foreign_producer_paths_and_path_escape(self):
        for stage in ("Frontend", "Validation"):
            write(self.sources[stage] / "evidence/duplicate.json", {})
            self.refresh(stage)
        with self.assertRaisesRegex(ValueError, "Unexpected Frontend producer path"):
            self.merge()
        path = self.sources["Package"] / "stage-manifest.json"
        manifest = json.loads(path.read_text())
        manifest["Files"]["../../escaped.exe"] = "f" * 64
        write(path, manifest)
        with self.assertRaisesRegex(ValueError, "catalog"):
            stages.verify(self.sources["Package"], "Package", COMMIT, "123", TARGET, "1")


if __name__ == "__main__":
    unittest.main()
