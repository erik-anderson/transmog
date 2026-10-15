"""Exercise architecture selection and signed updater asset boundaries."""
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

from test_tools import load
import repo_tools


class WindowsArm64ReleaseTests(unittest.TestCase):
    def test_updater_signs_only_the_installer_for_the_requested_architecture(self):
        module = load("create-update-manifest")
        for target, architecture, platform in (
            ("x86_64-pc-windows-msvc", "x64", "windows-x86_64-nsis"),
            ("aarch64-pc-windows-msvc", "arm64", "windows-aarch64-nsis"),
        ):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                installer = root / f"Transmog_1.2.3_{architecture}-setup.exe"
                installer.write_bytes(b"signed installer fixture")
                def sign(arguments, **kwargs):
                    self.assertEqual(arguments[-1], installer)
                    installer.with_suffix(".exe.sig").write_text("signed fixture", encoding="utf-8")
                    return type("Result", (), {"returncode": 0})()
                argv = ["create-update-manifest", "--target", target, "--installer", str(installer), "--version", "1.2.3", "--output-directory", str(root)]
                with patch.object(sys, "argv", argv), patch.dict(os.environ, {"TAURI_SIGNING_PRIVATE_KEY": "fixture"}), patch.object(module, "command", side_effect=sign) as signer:
                    module.main()
                    update = json.loads((root / "latest.json").read_text(encoding="utf-8"))
                    self.assertEqual(list(update["platforms"]), [platform])
                    self.assertEqual(update["platforms"][platform], {
                        "url": f"https://github.com/erik-anderson/transmog/releases/download/v1.2.3/{installer.name}",
                        "signature": "signed fixture",
                    })
                    argv[2] = "aarch64-pc-windows-msvc" if architecture == "x64" else "x86_64-pc-windows-msvc"
                    with self.assertRaisesRegex(ValueError, "Unexpected updater installer"):
                        module.main()
                    self.assertEqual(signer.call_count, 1)

    def test_v8_archives_are_verified_and_cached_separately(self):
        module = load("cache-v8")
        for target in module.ARCHIVES:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                payload = ("archive for " + target).encode()
                argv = ["cache-v8", "--target", target]
                with patch.object(sys, "argv", argv), patch.dict(os.environ, {"CARGO_HOME": temporary}), patch.dict(module.ARCHIVES, {target: hashlib.sha256(payload).hexdigest()}), patch.object(module, "urlopen", return_value=io.BytesIO(payload)) as download:
                    module.main()
                    self.assertIn(target + ".lib.gz", download.call_args.args[0])
                    cached = list((Path(temporary) / ".rusty_v8").iterdir())
                    self.assertEqual(len(cached), 1)
                    self.assertEqual(cached[0].read_bytes(), payload)
                    self.assertIn(target.replace("-", "_"), cached[0].name)
                    cached[0].write_bytes(b"wrong architecture or changed bytes")
                    with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
                        module.main()

    def test_arm64_native_environment_uses_arm64_sdk_and_linker(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            program_files = root / "Program Files (x86)"
            vswhere = program_files / "Microsoft Visual Studio/Installer/vswhere.exe"
            vswhere.parent.mkdir(parents=True)
            vswhere.touch()
            vs = root / "Visual Studio"
            vc = vs / "VC/Tools/MSVC/14.44.1"
            vc.mkdir(parents=True)
            sdk = program_files / "Windows Kits/10"
            (sdk / "Include/10.0.26100.0").mkdir(parents=True)
            environment = {"PATH": "fixture", "ProgramFiles(x86)": str(program_files)}
            result = type("Result", (), {"stdout": str(vs)})()
            with patch.object(repo_tools.host_platform, "machine", return_value="ARM64"), patch.object(repo_tools.subprocess, "run", return_value=result) as discover, patch.object(repo_tools, "executable", side_effect=lambda name, env: str(root / name)):
                configured = repo_tools.development_environment(root=root, environment=environment, platform="win32")
            self.assertIn("Microsoft.VisualStudio.Component.VC.Tools.ARM64", discover.call_args.args[0])
            self.assertIn(str(vc / "bin/Hostarm64/arm64"), configured["PATH"])
            self.assertEqual(configured["LIB"].split(";"), list(map(str, [vc / "lib/arm64", sdk / "Lib/10.0.26100.0/ucrt/arm64", sdk / "Lib/10.0.26100.0/um/arm64"])))
            self.assertEqual(configured["CFLAGS"], "--target=aarch64-pc-windows-msvc")
            self.assertNotIn("LIB", environment)


if __name__ == "__main__":
    unittest.main()
