"""Verify target libraries, compiler compatibility and cross-host build transfer."""
import copy
import json
import os
from pathlib import Path
import struct
import subprocess
import shutil
import sys
import tempfile
import unittest
import zipfile
from unittest.mock import patch

from test_release_stages import coff_library
import v8_artifact as engine
import windows_compiled as compiled
import windows_cross as cross

COMMIT = "a" * 40


def sdk(root, architecture="x64"):
    vc = root / "VC/Tools/MSVC/14.51.36231"
    kit = root / "Windows Kits/10"
    includes = [vc / "include", *[kit / "Include/10.0.26100.0" / part for part in ("ucrt", "shared", "um", "winrt")]]
    markers = (("vcruntime.h", "yvals_core.h"), ("corecrt.h",), ("sdkddkver.h",), ("windows.h",), ())
    for directory, names in zip(includes, markers):
        directory.mkdir(parents=True)
        for name in names:
            (directory / name).write_text(name)
    for arch in ("x64", "arm64"):
        for directory, names in ((vc / "lib" / arch, ("libcmt.lib", "msvcrt.lib", "vcruntime.lib")),
                                 (kit / "Lib/10.0.26100.0/ucrt" / arch, ("ucrt.lib",)),
                                 (kit / "Lib/10.0.26100.0/um" / arch, ("kernel32.lib",))):
            directory.mkdir(parents=True)
            for name in names:
                (directory / name).write_text(arch + name)
    libraries = [vc / "lib" / architecture, kit / "Lib/10.0.26100.0/ucrt" / architecture, kit / "Lib/10.0.26100.0/um" / architecture]
    return {"INCLUDE": ";".join(map(str, includes)), "LIB": ";".join(map(str, libraries)), "PATH": "native-tools"}


def versions(host, environment):
    return dict(environment, rustc="rustc pinned\ncommit-hash: abc\nhost: " + host + "\nLLVM version: pinned",
                clang="clang version pinned\nTarget: " + host + "\nThread model: posix\nInstalledDir: native-tools")


class CrossToolchainTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.environment = sdk(self.root / "producer")

    def test_cross_environment_preserves_host_libraries_and_scopes_target_tools(self):
        original = dict(self.environment)
        with patch.object(cross.shutil, "which", side_effect=lambda name, **_: "C:/native-tools/" + name + ".exe"):
            cross.configure(self.environment, cross.ARM64, root=self.root, host=cross.X64)
        self.assertEqual(self.environment["LIB"], original["LIB"])
        self.assertEqual(self.environment["PATH"], original["PATH"])
        self.assertEqual(self.environment["CARGO_BUILD_TARGET"], cross.ARM64)
        flags = self.environment["CARGO_ENCODED_RUSTFLAGS"].split("\x1f")
        self.assertEqual(flags, ["-Lnative=" + str(path) for path in cross.target_libraries(original, cross.ARM64)])
        self.assertNotIn("CMAKE_TOOLCHAIN_FILE", self.environment)
        toolchain = Path(self.environment["CMAKE_TOOLCHAIN_FILE_aarch64_pc_windows_msvc"]).read_text()
        self.assertIn("set(CMAKE_SYSTEM_PROCESSOR ARM64)", toolchain)
        self.assertIn("aarch64-pc-windows-msvc", toolchain)
        self.assertIn("/LIBPATH:", toolchain)
        first = dict(self.environment)
        with patch.object(cross.shutil, "which", side_effect=lambda name, **_: "C:/native-tools/" + name + ".exe"):
            cross.configure(self.environment, cross.ARM64, root=self.root, host=cross.X64)
        self.assertEqual(self.environment, first)

    def test_native_environment_does_not_change_and_reverse_cross_build_is_rejected(self):
        original = dict(self.environment)
        cross.configure(self.environment, cross.X64, root=self.root, host=cross.X64)
        self.assertEqual(self.environment, original)
        with self.assertRaisesRegex(ValueError, "x64 host"):
            cross.configure(self.environment, cross.X64, root=self.root, host=cross.ARM64)

    def test_existing_encoded_flags_and_windows_paths_are_preserved(self):
        self.environment["RUSTFLAGS"] = r"-Clink-arg=C:\owned\library.lib"
        with patch.object(cross.shutil, "which", return_value="C:/native-tools/compiler.exe"):
            cross.configure(self.environment, cross.ARM64, root=self.root, host=cross.X64)
        self.assertEqual(self.environment["CARGO_ENCODED_RUSTFLAGS"].split("\x1f")[0], r"-Clink-arg=C:\owned\library.lib")

    def test_missing_arm64_toolset_fails_before_compilation(self):
        path = cross.target_libraries(self.environment, cross.ARM64)[0]
        for child in path.iterdir():
            child.unlink()
        path.rmdir()
        with self.assertRaisesRegex(ValueError, "Visual Studio ARM64"):
            cross.configure(self.environment, cross.ARM64, root=self.root, host=cross.X64)

    def test_cross_and_native_tools_have_identical_target_identity(self):
        native = sdk(self.root / "consumer", "arm64")
        producer = versions(cross.X64, self.environment)
        consumer = versions(cross.ARM64, native)
        with patch.object(engine, "raw_tools", return_value=producer):
            cross_identity = engine.tools(cross.ARM64)
        with patch.object(engine, "raw_tools", return_value=consumer):
            self.assertEqual(engine.tools(cross.ARM64), cross_identity)
            library = cross.target_libraries(native, cross.ARM64)[0] / "vcruntime.lib"
            library.write_bytes(b"different target runtime")
            self.assertNotEqual(engine.tools(cross.ARM64), cross_identity)
        changed = dict(consumer, clang=consumer["clang"].replace("version pinned", "version changed"))
        self.assertNotEqual(engine.compiler_versions(changed), engine.compiler_versions(producer))

    def test_abi_accepts_only_redundant_target_defaults_and_verified_library_searches(self):
        environment = dict(self.environment, CFLAGS="--target=" + cross.ARM64, CXXFLAGS="--target=" + cross.ARM64)
        paths = cross.target_libraries(environment, cross.ARM64)
        environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join("-Lnative=" + str(path) for path in paths)
        with patch.dict(os.environ, {name: "" for name in engine.ABI_ENV}), patch.dict(os.environ, environment):
            normalized = engine.abi(cross.ARM64)
            self.assertEqual(normalized["CFLAGS"], "")
            self.assertEqual(normalized["CARGO_ENCODED_RUSTFLAGS"], "")
            os.environ["CARGO_ENCODED_RUSTFLAGS"] += "\x1f-Ctarget-feature=+crt-static"
            self.assertEqual(engine.abi(cross.ARM64)["CARGO_ENCODED_RUSTFLAGS"], "-Ctarget-feature=+crt-static")
            os.environ["CFLAGS"] = "--target=" + cross.X64
            self.assertNotEqual(engine.abi(cross.ARM64)["CFLAGS"], "")

    def test_archive_target_checks_every_member_and_truncated_archives(self):
        path = self.root / "engine.lib"
        path.write_bytes(coff_library(0xAA64))
        engine.archive_target(path, cross.ARM64)
        with self.assertRaisesRegex(ValueError, "architecture"):
            engine.archive_target(path, cross.X64)
        path.write_bytes(coff_library(0xAA64) + coff_library(0x8664)[8:])
        with self.assertRaisesRegex(ValueError, "architecture"):
            engine.archive_target(path, cross.ARM64)
        path.write_bytes(coff_library(0xAA64)[:-1])
        with self.assertRaisesRegex(ValueError, "Truncated"):
            engine.archive_target(path, cross.ARM64)


class CompiledTransferTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.producer = self.root / "producer"
        self.consumer = self.root / "consumer"
        self.bundle = self.root / "compiled"
        self.engine = self.root / "engine"
        self.engine.mkdir()
        identity = {"target": cross.ARM64, "inputs": {"source": "reviewed"}}
        self.engine_manifest = {"identity": identity, "key": engine.encoded_digest(identity)}
        (self.engine / "manifest.json").write_text(json.dumps(self.engine_manifest))
        self.env = patch.dict(os.environ, {"GITHUB_SHA": COMMIT, "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1",
                                          "TRANSMOG_V8_ARTIFACT": str(self.engine), "V8_ARTIFACT_ID": "100"})
        self.env.start()
        self.addCleanup(self.env.stop)
        binaries = self.producer / "binaries"
        binaries.mkdir(parents=True)
        for name in compiled.BINARIES:
            data = bytearray(70)
            data[:2] = b"MZ"
            struct.pack_into("<I", data, 60, 64)
            data[64:68] = b"PE\0\0"
            struct.pack_into("<H", data, 68, 0xAA64)
            (binaries / name).write_bytes(data)
        for name in compiled.PDBS:
            (binaries / name).write_bytes(b"original symbols")
        gate = {"Name": "Build", "Stage": "Package", "Result": "passed", "Commit": COMMIT,
                "Target": cross.ARM64, "RunId": "123", "RunAttempt": "1"}
        self.state = {"Commit": COMMIT, "RunId": "123", "RunAttempt": "1", "Phases": {"Build": gate},
                      "Build": {"BuildDirectory": str(binaries), "Target": cross.ARM64, "Version": "0.1.20", "Channel": "Canary"}}
        (self.producer / "artifacts").mkdir()
        (self.producer / "artifacts/windows-build-state.json").write_text(json.dumps(self.state))
        for root in (self.producer, self.consumer):
            (root / "artifacts/windows-build/evidence").mkdir(parents=True)
        for name in ("THIRD_PARTY_NOTICES.md", "sbom.cdx.json"):
            (self.producer / "artifacts/windows-build/evidence" / name).write_text(name)
        ui = self.producer / "apps/desktop/ui/dist"
        ui.mkdir(parents=True)
        (ui / "index.html").write_text("compiled UI")
        with patch.object(compiled.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "host: " + cross.X64)):
            compiled.export(self.bundle, cross.ARM64, root=self.producer)

    def test_original_binaries_symbols_ui_engine_and_prior_attempt_survive_transfer(self):
        os.environ["GITHUB_RUN_ATTEMPT"] = "2"
        build = compiled.restore(self.bundle, cross.ARM64, "101", root=self.consumer)
        self.assertEqual(Path(build["BuildDirectory"]), self.bundle / "binaries")
        self.assertEqual((self.consumer / "apps/desktop/ui/dist/index.html").read_text(), "compiled UI")
        proof = json.loads((self.consumer / "artifacts/windows-build/evidence/compiled-build.json").read_text())
        self.assertEqual(proof["Manifest"]["Gate"]["RunAttempt"], "1")
        self.assertEqual(proof["ArtifactId"], "101")
        for name in (*compiled.BINARIES, *compiled.PDBS):
            self.assertEqual((self.bundle / "binaries" / name).read_bytes(), (self.producer / "binaries" / name).read_bytes())

    def test_tampered_bytes_and_wrong_architecture_are_rejected_before_restoring(self):
        binary = self.bundle / "binaries/transmog.exe"
        original = binary.read_bytes()
        binary.write_bytes(original + b"tampered")
        with self.assertRaisesRegex(ValueError, "checksum"):
            compiled.restore(self.bundle, cross.ARM64, "101", root=self.consumer)
        self.assertFalse((self.consumer / "apps/desktop/ui/dist").exists())
        binary.write_bytes(original[:68] + struct.pack("<H", 0x8664))
        manifest = json.loads((self.bundle / "manifest.json").read_text())
        manifest["Files"] = compiled.catalog(self.bundle)
        (self.bundle / "manifest.json").write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, "architecture"):
            compiled.restore(self.bundle, cross.ARM64, "101", root=self.consumer)

    def qualified_payload(self):
        compiled.restore(self.bundle, cross.ARM64, "101", root=self.consumer)
        payload = self.consumer / "artifacts/windows-build"
        runtime = payload / "target/release"
        runtime.mkdir(parents=True)
        for name in compiled.BINARIES:
            (runtime / name).write_bytes((self.bundle / "binaries" / name).read_bytes())
        ui = payload / "apps/desktop/ui/dist"
        ui.mkdir(parents=True)
        (ui / "index.html").write_text("compiled UI")
        symbols = payload / "evidence/Transmog_0.1.20_windows-arm64-symbols.zip"
        def write_symbols(changed=False):
            with zipfile.ZipFile(symbols, "w") as archive:
                for name in compiled.PDBS:
                    archive.writestr(name, b"changed" if changed else (self.bundle / "binaries" / name).read_bytes())
        write_symbols()
        record = {"ArtifactId": "100", "Engine": self.engine_manifest,
                  "ManifestSha256": engine.digest(self.engine / "manifest.json").upper()}
        (payload / "evidence/v8-engine-package.json").write_text(json.dumps(record))
        return payload, write_symbols

    def test_qualification_rechecks_original_runtime_ui_notices_and_full_symbols(self):
        payload, write_symbols = self.qualified_payload()
        runtime = payload / "target/release"
        ui = payload / "apps/desktop/ui/dist"
        compiled.verify_qualified(payload, cross.ARM64, COMMIT, "123", "1")
        for path in (runtime / "transmog.exe", ui / "index.html", payload / "evidence/sbom.cdx.json"):
            original = path.read_bytes()
            path.write_bytes(original + b"changed")
            with self.subTest(path=path), self.assertRaisesRegex(ValueError, "changed a compiled payload"):
                compiled.verify_qualified(payload, cross.ARM64, COMMIT, "123", "1")
            path.write_bytes(original)
        write_symbols(changed=True)
        with self.assertRaisesRegex(ValueError, "changed compiled symbols"):
            compiled.verify_qualified(payload, cross.ARM64, COMMIT, "123", "1")

    @unittest.skipUnless(os.name == "nt" and shutil.which("pwsh"), "Windows PowerShell 7 required")
    def test_signing_boundary_rejects_changed_compiled_bytes_even_with_a_fresh_payload_catalog(self):
        payload, _ = self.qualified_payload()
        record = json.loads((payload / "evidence/v8-engine-package.json").read_text())
        (payload / "evidence/v8-engine-validation.json").write_text(json.dumps(record))
        stages = {"Frontend": ("UI", "Browser"), "Validation": ("Analysis", "Tests", "Native", "Archives"), "Package": ("Build", "Qualify")}
        producers = {}
        for index, (stage, gates) in enumerate(stages.items(), 102):
            producers[stage] = {"ArtifactId": str(index), "ManifestSha256": "e" * 64, "RunAttempt": "1"}
            for name in gates:
                path = payload / "evidence/gates" / (name + ".json")
                path.parent.mkdir(exist_ok=True)
                path.write_text(json.dumps({"Name": name, "Stage": stage, "Result": "passed", "Commit": COMMIT,
                                            "Target": cross.ARM64, "RunId": "123", "RunAttempt": "1"}))
        (payload / "evidence/pipeline-inputs.json").write_text(json.dumps({"Commit": COMMIT, "RunId": "123",
            "RunAttempt": "1", "Target": cross.ARM64, "Stages": producers, "V8": record}))
        manifest = {"Commit": COMMIT, "RunId": "123", "Target": cross.ARM64, "SourceBranch": "main",
                    "Version": "0.1.20", "Channel": "Canary", "ReleaseType": "Canary", "PipelineSchema": 1}
        def refresh():
            manifest["Files"] = [{"Path": path.relative_to(payload).as_posix(), "Sha256": engine.digest(path)}
                                 for path in payload.rglob("*") if path.is_file() and path.name != "build-manifest.json"]
            (payload / "build-manifest.json").write_text(json.dumps(manifest))
        refresh()
        script = self.root / "signing-boundary.ps1"
        script.write_text("param([string]$Common,[string]$Payload)\n. $Common\n"
                          "Assert-ReleasePayload -PayloadRoot $Payload -Commit '" + COMMIT + "' -RunId '123' | Out-Null\n")
        command = [shutil.which("pwsh"), "-NoProfile", "-File", str(script),
                   str(Path(compiled.__file__).parent / "windows-release-common.ps1"), str(payload)]
        environment = dict(os.environ, TRANSMOG_PYTHON=sys.executable, PYTHONUTF8="1")
        result = subprocess.run(command, env=environment, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        binary = payload / "target/release/transmog.exe"
        binary.write_bytes(binary.read_bytes() + b"changed after native qualification")
        refresh()
        result = subprocess.run(command, env=environment, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("compiled payload", result.stdout + result.stderr)

    def test_mismatched_source_run_attempt_target_gate_or_engine_cannot_qualify(self):
        manifest = json.loads((self.bundle / "manifest.json").read_text())
        for name, value in (("Commit", "b" * 40), ("RunId", "124"), ("RunAttempt", "2"), ("Target", cross.X64), ("Host", cross.ARM64)):
            changed = dict(manifest, **{name: value})
            with self.subTest(name=name), self.assertRaises(ValueError):
                compiled.verify_manifest(changed, cross.ARM64, COMMIT, "123", "1")
        changed = copy.deepcopy(manifest)
        changed["Gate"]["Result"] = "failed"
        with self.assertRaisesRegex(ValueError, "owning build gate"):
            compiled.verify_manifest(changed, cross.ARM64, COMMIT, "123", "1")
        os.environ["V8_ARTIFACT_ID"] = "999"
        with self.assertRaisesRegex(ValueError, "different engines"):
            compiled.restore(self.bundle, cross.ARM64, "101", root=self.consumer)

    def test_manifest_paths_and_immutable_evidence_are_checked(self):
        manifest = json.loads((self.bundle / "manifest.json").read_text())
        for name in ("ui/../../outside", "ui/C:/outside", "ui\\outside"):
            changed = copy.deepcopy(manifest)
            changed["Files"][name] = "a" * 64
            with self.subTest(name=name), self.assertRaises(ValueError):
                compiled.verify_manifest(changed, cross.ARM64, COMMIT, "123", "1")
        evidence = {"ArtifactId": "101", "ManifestCanonicalSha256": "0" * 64, "Manifest": manifest}
        with self.assertRaisesRegex(ValueError, "digest"):
            compiled.verify_evidence(evidence, cross.ARM64, COMMIT, "123", "1")


if __name__ == "__main__":
    unittest.main()
