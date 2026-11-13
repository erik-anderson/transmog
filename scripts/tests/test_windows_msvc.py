"""An image rollout must not change the hosted MSVC consumer profile."""
import ast
import collections
import glob
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import repo_tools
import v8_artifact
import v8_native


class WindowsMsvcTests(unittest.TestCase):
    def fixture(self, root, architecture, *, private=True):
        programs = root / "programs"
        vswhere = programs / "Microsoft Visual Studio/Installer/vswhere.exe"
        vswhere.parent.mkdir(parents=True)
        vswhere.touch()
        installation = root / "image-visual-studio"
        for version in ("14.51.36231", "14.52.37000"):
            (installation / "VC/Tools/MSVC" / version).mkdir(parents=True)
        sdk = programs / "Windows Kits/10"
        for version in ("10.0.26100.0", "10.0.26586.0"):
            for part in ("ucrt", "shared", "um", "winrt"):
                (sdk / "Include" / version / part).mkdir(parents=True)
        for target in ("x64", "arm64"):
            for part in ("ucrt", "um"):
                (sdk / "Lib/10.0.26100.0" / part / target).mkdir(parents=True)
        vc = root / ".tools/msvc/VC/Tools/MSVC/14.51.36231"
        if private:
            host = "Hostarm64" if architecture == "arm64" else "Hostx64"
            linker = vc / f"bin/{host}/{architecture}/link.exe"
            linker.parent.mkdir(parents=True)
            linker.touch()
            for target in ("x64", "arm64"):
                directory = vc / "lib" / target
                directory.mkdir(parents=True)
                for name in ("libcmt.lib", "msvcrt.lib", "vcruntime.lib"):
                    (directory / name).write_bytes(name.encode())
            for directory, names in ((vc / "include", ("vcruntime.h", "yvals_core.h")),
                    (sdk / "Include/10.0.26100.0/ucrt", ("corecrt.h",)),
                    (sdk / "Include/10.0.26100.0/shared", ("sdkddkver.h",)),
                    (sdk / "Include/10.0.26100.0/um", ("windows.h",))):
                directory.mkdir(parents=True, exist_ok=True)
                for name in names:
                    (directory / name).write_bytes(name.encode())
            for target in ("x64", "arm64"):
                (sdk / "Lib/10.0.26100.0/ucrt" / target / "ucrt.lib").write_bytes(b"ucrt")
                (sdk / "Lib/10.0.26100.0/um" / target / "kernel32.lib").write_bytes(b"kernel32")
        return {"PATH": "", "ProgramFiles(x86)": str(programs)}, installation, vc

    def configure(self, root, environment, installation, architecture, target=None):
        with patch.object(repo_tools.host_platform, "machine", return_value=architecture), \
             patch.object(repo_tools.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, str(installation))), \
             patch.object(repo_tools, "executable", side_effect=lambda name, env: str(root / (name + ".exe"))), \
             patch("windows_cross.shutil.which", side_effect=lambda name, path: str(root / (name + ".exe"))):
            return repo_tools.development_environment(root=root, environment=environment, platform="win32", target=target)

    def test_private_profile_wins_over_newer_image_on_both_native_hosts(self):
        fingerprints = []
        for architecture in ("x64", "arm64"):
            with self.subTest(architecture=architecture), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                env, image, vc = self.fixture(root, architecture)
                configured = self.configure(root, env, image, architecture)
                self.assertEqual(configured["VCToolsVersion"], "14.51.36231")
                self.assertTrue(configured["INCLUDE"].startswith(str(vc / "include") + ";"))
                self.assertNotIn("14.52", configured["PATH"])
                self.assertNotIn("26586", configured["INCLUDE"])
                suffix = "AARCH64_PC_WINDOWS_MSVC" if architecture == "arm64" else "X86_64_PC_WINDOWS_MSVC"
                self.assertTrue(Path(configured["CARGO_TARGET_" + suffix + "_LINKER"]).is_file())
                fingerprints.append(v8_artifact.sdk_identity("aarch64-pc-windows-msvc", configured))
                self.assertEqual(self.configure(root, configured, image, architecture), configured)
                self.assertNotIn("VCToolsVersion", env)
        self.assertEqual(*fingerprints)

    def test_real_gn_discovery_uses_the_same_private_profile_over_newer_image_hints(self):
        pin = v8_native.source_pin()
        archive = v8_native.source_archive(pin)
        with tarfile.open(archive) as source:
            code = source.extractfile(f'v8-{pin["version"]}/build/vs_toolchain.py').read().decode()
        tree = ast.parse(code)
        functions = {"_GenerateCandidatePaths", "_CheckIfVersionInstalled", "GetVisualStudioVersion",
                     "DetectVisualStudioPath", "_HostIsWindows", "SetEnvironmentAndGetRuntimeDllDirs",
                     "_SortByHighestVersionNumberFirst", "FindVCComponentRoot"}
        nodes = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name in functions or
                 isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == "MSVS_VERSIONS" for t in node.targets)]
        namespace = {"os": os, "glob": glob, "re": re, "collections": collections,
                     "sys": SimpleNamespace(platform="win32"),
                     "platform": SimpleNamespace(architecture=lambda: ("64bit", "")), "json_data_file": "unused"}
        # Execute the pinned archive's actual discovery functions, not a copy of
        # their logic. No compiler, vcvarsall or application is launched.
        exec(compile(ast.Module(body=nodes, type_ignores=[]), "pinned-v8/build/vs_toolchain.py", "exec"), namespace)
        for architecture in ("x64", "arm64"):
            with self.subTest(architecture=architecture), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                env, image, vc = self.fixture(root, architecture)
                env.update(DEPOT_TOOLS_WIN_TOOLCHAIN="0", vs2026_install=str(image),
                           GYP_MSVS_OVERRIDE_PATH=str(image), WINDOWSSDKDIR="unrelated-sdk")
                # Before setup, GN independently chooses the image's newer tree.
                with patch.dict(os.environ, env, clear=True):
                    self.assertEqual(Path(namespace["FindVCComponentRoot"]("Tools")).name, "14.52.37000")
                configured = self.configure(root, env, image, architecture)
                with patch.dict(os.environ, configured, clear=True):
                    self.assertEqual(Path(namespace["FindVCComponentRoot"]("Tools")), vc)
                    self.assertEqual(Path(namespace["DetectVisualStudioPath"]()), root / ".tools/msvc")
                    self.assertEqual(Path(os.environ["WINDOWSSDKDIR"]), root / "programs/Windows Kits/10")

    def test_partial_private_installation_fails_without_image_fallback(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env, image, vc = self.fixture(root, "arm64")
            (vc / "bin/Hostarm64/arm64/link.exe").unlink()
            with self.assertRaisesRegex(RuntimeError, "fixed MSVC installation is incomplete"):
                self.configure(root, env, image, "arm64")

    def test_private_profile_rejects_missing_fixed_sdk(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env, image, _ = self.fixture(root, "x64")
            (root / "programs/Windows Kits/10/Include/10.0.26100.0").rename(
                root / "programs/Windows Kits/10/Include/10.0.26100.99")
            with self.assertRaisesRegex(RuntimeError, "fixed Windows SDK"):
                self.configure(root, env, image, "x64")

    def test_cross_target_keeps_native_linker_and_separate_arm64_libraries(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env, image, vc = self.fixture(root, "x64")
            configured = self.configure(root, env, image, "x64", "aarch64-pc-windows-msvc")
            self.assertIn(str(vc / "lib/x64"), configured["LIB"])
            self.assertIn("-Lnative=" + str(vc / "lib/arm64"), configured["CARGO_ENCODED_RUSTFLAGS"])
            self.assertIn("Hostx64", configured["CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER"])
            self.assertEqual(configured["CARGO_BUILD_TARGET"], "aarch64-pc-windows-msvc")

    def test_local_checkouts_without_private_installation_keep_existing_discovery(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env, image, _ = self.fixture(root, "x64", private=False)
            configured = self.configure(root, env, image, "x64")
            self.assertIn("14.52.37000", configured["INCLUDE"])
            self.assertNotIn("VCToolsVersion", configured)

    def test_image_toolset_rollover_reproduces_identity_rejection_and_names_the_difference(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env, image, vc = self.fixture(root, "arm64")
            # This is the original latest-directory selection against an image
            # containing both the engine's 14.51 and the new default 14.52.
            selected = max((image / "VC/Tools/MSVC").iterdir(), key=repo_tools.version_key)
            self.assertEqual(selected.name, "14.52.37000")
            configured = self.configure(root, env, image, "arm64")
            expected = {"tools": {"windows": v8_artifact.sdk_identity("aarch64-pc-windows-msvc", configured)}}
            actual = json.loads(json.dumps(expected).replace("MSVC/14.51.36231/", "MSVC/14.52.37000/"))
            bundle = root / "engine"
            bundle.mkdir()
            for name in v8_artifact.FILES:
                (bundle / name).touch()
            (bundle / "manifest.json").write_text(json.dumps({"identity": expected}))
            with patch.object(v8_artifact, "identity", return_value=actual):
                with self.assertRaisesRegex(ValueError, "compiler differs:.*MSVC/14.51.36231/include"):
                    v8_artifact.validate(bundle, "aarch64-pc-windows-msvc")
            self.assertEqual(v8_artifact.identity_differences(expected, expected), [])
            self.assertEqual(v8_artifact.identity_differences({"extra": None}, {}), ["identity.extra"])


if __name__ == "__main__":
    unittest.main()
