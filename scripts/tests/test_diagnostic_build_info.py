import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import repo_tools


def git(root, *arguments):
    return subprocess.run(["git", "-C", str(root), *arguments], check=True,
                          capture_output=True, env=dict(os.environ, GIT_OPTIONAL_LOCKS="0"))


def commit(root):
    git(root, "-c", "user.name=Build metadata tests", "-c", "user.email=metadata@example.invalid",
        "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-qm", "Build fixture")


class SourceStateTests(unittest.TestCase):
    def test_commit_and_untracked_changes_invalidate_but_ignored_outputs_do_not(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            git(root, "init", "-q")
            (root / ".gitignore").write_text("ignored/\n")
            git(root, "add", ".")
            commit(root)
            first = repo_tools.source_state(root, dict(os.environ))
            self.assertEqual(len(first), 64)
            (root / "ignored").mkdir()
            (root / "ignored/output").write_text("output")
            self.assertEqual(first, repo_tools.source_state(root, dict(os.environ)))
            (root / "source").write_text("new source")
            self.assertNotEqual(first, repo_tools.source_state(root, dict(os.environ)))
            (root / "source").unlink()
            self.assertEqual(first, repo_tools.source_state(root, dict(os.environ)))
            commit(root)
            self.assertNotEqual(first, repo_tools.source_state(root, dict(os.environ)))

    def test_missing_git_metadata_is_unavailable(self):
        with tempfile.TemporaryDirectory() as temporary:
            self.assertEqual(repo_tools.source_state(Path(temporary), dict(os.environ)), "unavailable")

    def test_git_cannot_be_started_is_unavailable(self):
        for error in (PermissionError("Git execution denied"), FileNotFoundError("Git was removed")):
            with self.subTest(error=type(error).__name__), \
                 mock.patch.object(repo_tools.shutil, "which", return_value="git"), \
                 mock.patch.object(repo_tools.subprocess, "run", side_effect=error):
                self.assertEqual(repo_tools.source_state(repo_tools.ROOT, dict(os.environ)), "unavailable")

    def test_sdk_build_receives_invalidation_without_mutating_parent_environment(self):
        original = {"KEEP": "value"}
        with mock.patch.object(repo_tools, "executable", return_value="cargo"), \
             mock.patch.object(repo_tools, "source_state", return_value="state"), \
             mock.patch.object(repo_tools.subprocess, "run") as run:
            for arguments in (["check"], ["+stable", "--offline", "rustc"], ["doc", "--no-deps"],
                              ["rustdoc"], ["b"], ["c"], ["t"], ["r"], ["d"]):
                with self.subTest(arguments=arguments):
                    repo_tools.command(["cargo", *arguments], cwd=repo_tools.ROOT / "sdk", env=original)
                    snapshot = json.loads(run.call_args.kwargs["env"]["TRANSMOG_SOURCE_STATE"])
                    self.assertEqual(snapshot, {"root": str(repo_tools.ROOT.resolve()), "state": "state"})
        self.assertEqual(original, {"KEEP": "value"})


@unittest.skipUnless(os.environ.get("TRANSMOG_BUILD_INFO_INTEGRATION") == "1", "opt-in Cargo metadata integration")
class BuildInfoIntegrationTests(unittest.TestCase):
    def test_archives_git_rebuilds_locked_versions_and_asset_integrity(self):
        with tempfile.TemporaryDirectory(prefix="diagnostic build info ") as temporary:
            owner = Path(temporary)
            # An archive inside another checkout must not claim the parent's commit.
            git(owner, "init", "-q")
            commit(owner)
            root = owner / "archive"
            root.mkdir()
            package = root / "crates/probe"
            (root / ".gitignore").write_text("target/\n__pycache__/\n")
            (package / "src").mkdir(parents=True)
            paths = ["Cargo.toml", "Cargo.lock", "apps/desktop/Cargo.toml", "crates/app-webui/Cargo.toml",
                     "apps/desktop/ui/package-lock.json", "build-support/diagnostics.rs",
                     "third_party/boringssl-pki/manifest.json", "third_party/swc-ecma-parser/transmog-snapshot.json",
                     "third_party/swc-ecma-parser/.cargo_vcs_info.json", "third_party/chromium-certificate-policy/snapshot.json",
                     "third_party/chromium-root-store/snapshot.json", "third_party/chromium-suffix-list/snapshot.json",
                     "scripts/dev-env.ps1", "scripts/dev-env.py", "scripts/python-tools.ps1", "scripts/repo_tools.py",
                     "scripts/build_cache.py", "scripts/cargo_cache_usage.py"]
            for name in paths:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(repo_tools.ROOT / name, path)
            (root / "scripts/probe-cargo.ps1").write_text('''param([string]$Manifest, [string]$Target)
. (Join-Path $PSScriptRoot 'dev-env.ps1')
cargo run --quiet --offline --manifest-path $Manifest --target-dir $Target
exit $LASTEXITCODE
''')
            (package / "Cargo.toml").write_text('''[workspace]
[package]
name="diagnostic-build-info-probe"
version="0.0.0"
edition="2024"
[dependencies]
serde_json="=1.0.154"
[build-dependencies]
hex="=0.4.3"
serde_json="=1.0.154"
sha2="=0.11.1"
toml="=1.1.6"
''')
            (package / "build.rs").write_text('''#[path="../../build-support/diagnostics.rs"] mod diagnostics;
fn main(){let root=std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");diagnostics::application(&root);diagnostics::desktop(&root);}
''')
            (package / "src/main.rs").write_text('''fn main(){let app:serde_json::Value=serde_json::from_str(include_str!(concat!(env!("OUT_DIR"),"/build-info.json"))).unwrap();let host:serde_json::Value=serde_json::from_str(include_str!(concat!(env!("OUT_DIR"),"/host-build-info.json"))).unwrap();println!("{}",serde_json::json!({"app":app,"host":host}));}
''')
            dist = root / "apps/desktop/ui/dist"
            dist.mkdir()
            (dist / "app.js").write_bytes(b"built client")
            (dist / "client-assets.json").write_text(json.dumps([{"file": "app.js"}]))
            npm = json.loads((root / "apps/desktop/ui/package-lock.json").read_text())
            client = {"dependencies": {name: npm["packages"]["node_modules/" + name]["version"]
                                       for name in ("@microsoft/webui-framework", "@tauri-apps/api")},
                      "assets": [{"file": "app.js", "sha256": hashlib.sha256(b"built client").hexdigest()}]}
            (dist / "client-build-info.json").write_text(json.dumps(client))
            environment = dict(repo_tools.development_environment(), CARGO_PROFILE_DEV_DEBUG="0", CARGO_INCREMENTAL="0",
                               TRANSMOG_BUILD_CACHE_TRIM="0", CARGO_BUILD_JOBS="2")

            def build(success=True, source_root=root, powershell=False, snapshot_root=None):
                command = ["cargo", "run", "--quiet", "--offline", "--manifest-path",
                           str(source_root / "crates/probe/Cargo.toml"), "--target-dir", str(root / "target")]
                if powershell:
                    # The documented dot-source entry point must refresh on every Cargo call.
                    arguments = [repo_tools.executable("pwsh", environment), "-NoProfile", "-File",
                                 str(source_root / "scripts/probe-cargo.ps1"), str(source_root / "crates/probe/Cargo.toml"),
                                 str(root / "target")]
                    clean_environment = dict(environment)
                    clean_environment.pop("TRANSMOG_SOURCE_STATE", None)
                    result = subprocess.run(arguments, cwd=source_root / "crates/probe", env=clean_environment,
                                            capture_output=True, text=True, timeout=120)
                # Use the real portable runner's invalidation against the fixture repository.
                else:
                    with mock.patch.object(repo_tools, "ROOT", snapshot_root or source_root):
                        result = repo_tools.command(command, cwd=source_root / "crates/probe",
                                                    env=environment, capture=True, check=False)
                self.assertEqual(result.returncode == 0, success, result.stderr)
                return json.loads(result.stdout) if success else result.stderr

            archive = build()
            self.assertIsNone(archive["app"]["identity"]["sourceCommit"])
            self.assertIsNone(archive["app"]["identity"]["sourceDirty"])
            self.assertTrue(archive["app"]["identity"]["target"])
            self.assertTrue(archive["app"]["identity"]["rustc"].startswith("rustc "))
            self.assertNotIn("tauri", {item["name"] for item in archive["app"]["dependencies"]})
            self.assertIn("tauri", {item["name"] for item in archive["host"]["dependencies"]})
            git(root, "init", "-q")
            git(root, "add", ".")
            commit(root)
            (root / "Cargo.lock").touch()
            clean = build()
            first_commit = git(root, "rev-parse", "HEAD").stdout.decode().strip()
            self.assertEqual(clean["app"]["identity"]["sourceCommit"], first_commit)
            self.assertFalse(clean["app"]["identity"]["sourceDirty"])
            commit(root)
            self.assertNotEqual(build()["app"]["identity"]["sourceCommit"], first_commit)
            with tempfile.TemporaryDirectory(prefix="diagnostic linked worktree ") as linked:
                checkout = Path(linked) / "checkout"
                git(root, "worktree", "add", "--detach", "-q", str(checkout))
                identity = build(source_root=checkout)["app"]["identity"]
                self.assertFalse(identity["sourceDirty"])
                other_snapshot = build(source_root=checkout, snapshot_root=root)["app"]["identity"]
                self.assertEqual(other_snapshot["sourceCommit"], identity["sourceCommit"])
                self.assertIsNone(other_snapshot["sourceDirty"])
                commit(checkout)
                self.assertNotEqual(build(source_root=checkout)["app"]["identity"]["sourceCommit"],
                                    identity["sourceCommit"])
                untracked = checkout / "untracked-source.txt"
                untracked.write_text("new source")
                self.assertTrue(build(source_root=checkout)["app"]["identity"]["sourceDirty"])
                untracked.unlink()
                self.assertFalse(build(source_root=checkout)["app"]["identity"]["sourceDirty"])
                if os.name == "nt":
                    self.assertIsNone(build(source_root=checkout, powershell=True)["app"]["identity"]["sourceDirty"])
                    untracked.write_text("new source via PowerShell")
                    self.assertIsNone(build(source_root=checkout, powershell=True)["app"]["identity"]["sourceDirty"])
                    self.assertTrue(build(source_root=checkout)["app"]["identity"]["sourceDirty"])
                    untracked.unlink()
                    self.assertIsNone(build(source_root=checkout, powershell=True)["app"]["identity"]["sourceDirty"])
                    self.assertFalse(build(source_root=checkout)["app"]["identity"]["sourceDirty"])
            with (root / "Cargo.toml").open("a") as output:
                output.write("\n# source modification\n")
            self.assertTrue(build()["app"]["identity"]["sourceDirty"])
            receipt_path = root / "third_party/chromium-root-store/snapshot.json"
            receipt = json.loads(receipt_path.read_text())
            malformed = dict(receipt)
            del malformed["rootStoreVersion"]
            receipt_path.write_text(json.dumps(malformed))
            self.assertIn("rootStoreVersion", build(False))
            receipt_path.write_text(json.dumps(receipt))
            receipt_path = root / "third_party/swc-ecma-parser/transmog-snapshot.json"
            receipt = json.loads(receipt_path.read_text())
            malformed = json.loads(json.dumps(receipt))
            malformed["source"]["version"] = "0.0.0"
            receipt_path.write_text(json.dumps(malformed))
            self.assertIn("qualified SWC parser version differs", build(False))
            receipt_path.write_text(json.dumps(receipt))
            (dist / "app.js").write_bytes(b"tampered client")
            self.assertIn("client build metadata does not match", build(False))
            (dist / "app.js").write_bytes(b"built client")
            client["dependencies"]["@microsoft/webui-framework"] = "0.0.0"
            (dist / "client-build-info.json").write_text(json.dumps(client))
            self.assertIn("built client version differs", build(False))


if __name__ == "__main__":
    unittest.main()
