"""Exercise packaging's actual PowerShell parameter scopes without compiling the app."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(os.name == "nt", "Windows PowerShell packaging boundary")
class PackageTargetTests(unittest.TestCase):
    def test_each_requested_target_reaches_environment_credits_tauri_and_result(self):
        pwsh = shutil.which("pwsh")
        self.assertIsNotNone(pwsh, "PowerShell is required for Windows release fixtures")
        for target in ("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                scripts = root / "scripts"
                scripts.mkdir()
                for name in ("package-windows.ps1", "windows-release-target.ps1", "dev-env.ps1", "python-tools.ps1"):
                    shutil.copyfile(ROOT / "scripts" / name, scripts / name)
                # Keep the real PowerShell parameter/environment bridge. Only its native-tool
                # discovery is replaced, so this exercises dot-sourcing and native cmd argv.
                (scripts / "dev-env.py").write_text(
                    'import json,sys\nprint(json.dumps({"CARGO_BUILD_TARGET": '
                    'sys.argv[sys.argv.index("--target")+1] if "--target" in sys.argv else ""}))\n')
                (scripts / "release-version-common.ps1").write_text(
                    "function Get-ReleaseVersionState { param($Root) "
                    "[pscustomobject]@{ SemVer='0.1.20'; Version='0.1.20'; Channel='Canary' } }\n")
                desktop = root / "apps/desktop"
                bins = desktop / "ui/node_modules/.bin"
                bins.mkdir(parents=True)
                (desktop / "tauri.conf.json").write_text(json.dumps({"version": "0.1.20", "bundle": {
                    "windows": {"webviewInstallMode": {"type": "skip"}}}}))
                capture = root / "capture.py"
                capture.write_text('import json,os,sys\nwith open(os.environ["CAPTURE"],"a") as f: '
                                   'f.write(json.dumps({"args":sys.argv[1:],"target":os.environ.get("CARGO_BUILD_TARGET")})+"\\n")\n')
                for path, kind in ((bins / "tauri.cmd", "tauri"), (root / "npm.cmd", "npm")):
                    path.write_text(f'@echo off\r\n"{sys.executable}" "{capture}" {kind} %*\r\nexit /b %errorlevel%\r\n')
                log = root / "calls.jsonl"
                environment = dict(os.environ, PATH=str(root) + os.pathsep + os.environ["PATH"],
                                   TRANSMOG_PYTHON=sys.executable, CAPTURE=str(log), CARGO_BUILD_TARGET="")
                wrapper = root / "run.ps1"
                wrapper.write_text("$ErrorActionPreference='Stop'\n"
                    f"& '{scripts / 'package-windows.ps1'}' -Target '{target}' -UnsignedDevelopment -BuildOnly -SkipTests | ConvertTo-Json -Compress\n")
                result = subprocess.run([pwsh, "-NoProfile", "-File", str(wrapper)], env=environment,
                                        text=True, encoding="utf-8", capture_output=True, check=True)
                build = json.loads(result.stdout)
                calls = [json.loads(line) for line in log.read_text().splitlines()]
                self.assertEqual(build["Target"], target)
                tauri = next(call for call in calls if call["args"][0] == "tauri")
                self.assertEqual(tauri["target"], target)
                if target.startswith("aarch64"):
                    self.assertEqual(tauri["args"][tauri["args"].index("--target") + 1], target)
                    credits = next(call for call in calls if call["args"] == ["npm", "run", "build"])
                    self.assertEqual(credits["target"], target)
                else:
                    self.assertNotIn("--target", tauri["args"])
                self.assertIn("--locked", tauri["args"])
                self.assertIn("transmog-script-host", tauri["args"])
                self.assertFalse((root / "artifacts/windows-package/signing-config.json").exists())


if __name__ == "__main__":
    unittest.main()
