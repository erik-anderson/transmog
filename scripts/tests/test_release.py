"""Release behavior regressions, including real local Git event replays."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import Request

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from repo_tools import ROOT, command, write_json
from release_version import State, assert_source_branch, main_line_decision, next_version, post_release_decision, semver, set_version
from test_tools import load


class ReleaseDecisions(unittest.TestCase):
    def test_release_download_redirect_cannot_forward_api_credentials(self):
        from github_release import ReleaseRedirect
        request = Request("https://api.github.com/fixture", headers={"Authorization": "Bearer fixture-secret"})
        redirected = ReleaseRedirect().redirect_request(request, None, 302, "found", {}, "https://release-assets.example.invalid/file")
        self.assertIsNone(redirected.get_header("Authorization"))
        with self.assertRaisesRegex(RuntimeError, "non-HTTPS"):
            ReleaseRedirect().redirect_request(request, None, 302, "found", {}, "http://release-assets.example.invalid/file")

    def test_api_distinguishes_not_found_from_failure(self):
        from github_release import request
        for code in (404, 500):
            with patch("github_release.urlopen", side_effect=HTTPError("https://api.github.com/fixture", code, "fixture", {}, None)):
                if code == 404:
                    self.assertIsNone(request("https://api.github.com/fixture"))
                else:
                    with self.assertRaisesRegex(RuntimeError, "HTTP 500"):
                        request("https://api.github.com/fixture")

    def test_repeat_publication_and_stable_promotion(self):
        self.assertEqual(post_release_decision(State("1.2.7", "Canary", "Canary"), "1.2.7"), State("1.2.8", "Canary", "Canary"))
        self.assertIsNone(post_release_decision(State("1.2.8", "Canary", "Canary"), "1.2.7"))
        self.assertEqual(post_release_decision(State("1.2.8", "Release", "Beta"), "1.2.7", "release/1.2", "Stable"), State("1.2.8", "Release", "Stable"))
        self.assertIsNone(post_release_decision(State("1.2.8", "Release", "Stable"), "1.2.7", "release/1.2", "Beta"))
        with self.assertRaises(ValueError):
            post_release_decision(State("1.2.6", "Release", "Stable"), "1.2.7", "release/1.2", "Stable")

    def test_version_overflow_and_release_line_guards(self):
        for version in ("1.2", "1.2.3.4", "1.2.65536", "1.02.3", "v1.2.3", "1.2.3+4"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                semver(version)
        with self.assertRaises(ValueError):
            next_version("1.2.65535")
        with self.assertRaises(ValueError):
            main_line_decision(State("0.9.3", "Canary", "Canary"), "1.0.0")
        with self.assertRaises(ValueError):
            main_line_decision(State("0.65535.1", "Canary", "Canary"), "0.65535.0")
        self.assertEqual(main_line_decision(State("0.1.7", "Canary", "Canary"), "0.2.0"), State("0.3.0", "Canary", "Canary"))
        self.assertIsNone(main_line_decision(State("0.3.8", "Canary", "Canary"), "0.2.0"))
        for branch in ("release/0", "release/0.1", "release/0.00", "release/0.0.1"):
            with self.subTest(branch=branch), self.assertRaises(ValueError):
                assert_source_branch(branch, "0.0.1")

    def test_highest_stable_selection_pagination_and_idempotence(self):
        module = load("sync-stable-update-release")
        def release(version, identifier, prerelease=False):
            return dict(tag_name="v" + version, id=identifier, draft=False, prerelease=prerelease,
                        assets=[dict(name="latest.json"), dict(name=f"Transmog_{version}_x64-setup.exe.sig")])
        for latest_id in (9, 10):
            mutations = []
            def api(url, **kwargs):
                if kwargs.get("method") == "PATCH":
                    mutations.append((url, kwargs["data"]))
                    return {}
                if url.endswith("/latest"):
                    return dict(id=latest_id)
                if "page=2" in url:
                    return [release("0.1.9", 9), release("0.1.10", 10)]
                return [release(f"2.0.{index}", 100 + index, True) for index in range(100)]
            module.synchronize("fixture/repository", api)
            self.assertEqual(mutations, [] if latest_id == 10 else [("https://api.github.com/repos/fixture/repository/releases/10", {"make_latest": "true"})])

    def test_canary_feed_selects_highest_canary_and_keeps_beta_out(self):
        module = load("sync-canary-update-feed")
        base = "https://api.github.com/repos/fixture/repository"
        updates = {"0.2.0": b'{"version":"0.2.0"}', "0.2.1": b'{"version":"0.2.1"}'}
        manifest = lambda version, release_type: json.dumps({"Version": version, "ReleaseType": release_type,
                                                              "Commit": ("a" if version == "0.2.0" else "b") * 40}).encode()
        releases = [
            dict(tag_name="v0.2.0", id=20, prerelease=True, assets=[dict(name="release-manifest.json", id=200), dict(name="latest.json", id=201)]),
            dict(tag_name="v0.2.1", id=21, prerelease=True, assets=[dict(name="release-manifest.json", id=210), dict(name="latest.json", id=211)]),
            dict(tag_name="v9.0.0", id=90, prerelease=True, assets=[dict(name="release-manifest.json", id=900), dict(name="latest.json", id=901)]),
        ]
        mutations, uploads = [], []
        def api(url, **kwargs):
            if url == base + "/releases?per_page=100&page=1": return releases
            if url.endswith("page=2"): return []
            if url.endswith("/releases/tags/canary"): return None
            if url.endswith("/releases/assets/200"): return manifest("0.2.0", "Canary")
            if url.endswith("/releases/assets/201"): return updates["0.2.0"]
            if url.endswith("/releases/assets/210"): return manifest("0.2.1", "Canary")
            if url.endswith("/releases/assets/211"): return updates["0.2.1"]
            if url.endswith("/releases/assets/900"): return manifest("9.0.0", "Beta")
            if kwargs.get("method") == "POST" and url.endswith("/git/refs"):
                mutations.append((url, kwargs["data"]))
                return {}
            if kwargs.get("method") == "POST" and url.endswith("/releases"):
                mutations.append((url, kwargs["data"]))
                return {"upload_url": "https://uploads.github.com/feed{?name}", "assets": []}
            self.fail(f"Unexpected fixture request: {url}")
        module.synchronize("fixture/repository", api, lambda url, name, payload: uploads.append((url, name, payload)))
        self.assertEqual(mutations[0][1]["sha"], "b" * 40)
        self.assertEqual(uploads, [("https://uploads.github.com/feed", "latest.json", updates["0.2.1"])])


def fixture(root):
    for name in ("apps/desktop/src", "apps/desktop/ui", "fuzz/src"):
        (root / name).mkdir(parents=True, exist_ok=True)
    (root / "Cargo.toml").write_text('[workspace]\nmembers = ["apps/desktop"]\nresolver = "2"\n[workspace.package]\nversion = "0.1.0"\nedition = "2024"\n')
    (root / "apps/desktop/Cargo.toml").write_text('[package]\nname = "transmog-desktop"\nversion.workspace = true\nedition.workspace = true\n')
    (root / "apps/desktop/src/lib.rs").write_text("// Metadata fixture.\n")
    (root / "fuzz/Cargo.toml").write_text('[package]\nname = "transmog-fuzz-fixture"\nversion = "0.0.0"\nedition = "2024"\n[workspace]\n')
    (root / "fuzz/src/lib.rs").write_text("// Metadata fixture.\n")
    write_json(root / "release-version.json", State("0.1.0", "Canary", "Canary").to_dict())
    write_json(root / "apps/desktop/tauri.conf.json", dict(version="0.1.0"))
    write_json(root / "apps/desktop/ui/package.json", dict(name="@transmog/desktop-ui", version="0.1.0", private=True))
    write_json(root / "apps/desktop/ui/package-lock.json", dict(name="@transmog/desktop-ui", version="0.1.0", lockfileVersion=3,
        packages={"": dict(name="@transmog/desktop-ui", version="0.1.0")}))
    command(["cargo", "generate-lockfile", "--offline"], cwd=root, capture=True)
    command(["cargo", "generate-lockfile", "--manifest-path", "fuzz/Cargo.toml", "--offline"], cwd=root, capture=True)


class LocalReleaseFixtures(unittest.TestCase):
    def setUp(self):
        parent = ROOT / ".local/tooling-tests"
        parent.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix="release-", dir=parent)
        self.root = Path(self.temporary.name).resolve()
        self.assertTrue(self.root.is_relative_to(parent.resolve()))

    def tearDown(self):
        self.temporary.cleanup()

    def test_failed_lockfile_refresh_restores_exact_original_bytes(self):
        fixture(self.root)
        config = self.root / "apps/desktop/tauri.conf.json"
        config.write_bytes(b'\xef\xbb\xbf{\r\n  "version": "0.1.0"\r\n}\r\n')
        original = {p: p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        def run(argv, **kwargs):
            if argv[1] == "update":
                (self.root / "Cargo.lock").write_bytes(b"partial updated lockfile")
                raise RuntimeError("fixture lockfile failure")
            return command(argv, **kwargs)
        with self.assertRaisesRegex(RuntimeError, "fixture lockfile failure"):
            set_version(self.root, "0.1.1", run=run)
        self.assertEqual({p: p.read_bytes() for p in original}, original)

    def test_preflight_drafts_published_versions_and_reserved_lines(self):
        fixture(self.root)
        module = load("check-published-release-version")
        write_json(self.root / "release-version.json", State("1.2.7", "Release", "Beta").to_dict())
        for result in (None, dict(draft=True), dict(draft=False)):
            calls = []
            def api(url):
                calls.append(url)
                return result
            if result and not result["draft"]:
                with self.assertRaises(ValueError):
                    module.preflight(self.root, "fixture/repository", "release/1.2", api=api)
            else:
                module.preflight(self.root, "fixture/repository", "release/1.2", api=api)
            self.assertEqual(len(calls), 1)
        write_json(self.root / "release-version.json", State("1.2.7", "Canary", "Canary").to_dict())
        with self.assertRaisesRegex(ValueError, "reserved"):
            module.preflight(self.root, "fixture/repository", "main", api=lambda url: dict(ref="reserved"))

    def test_real_git_events_are_idempotent_and_keep_stable_track(self):
        module = load("advance-release-version")
        checkout, origin = self.root / "checkout", self.root / "origin.git"
        checkout.mkdir()
        command(["git", "init", "--bare", "-q", origin], capture=True)
        command(["git", "init", "--initial-branch=main", "-q", checkout], capture=True)
        def git(*argv):
            try:
                return command(["git", *argv], cwd=checkout, capture=True).stdout.strip()
            except subprocess.CalledProcessError as error:
                self.fail(error.stderr)
        git("config", "user.name", "Fixture")
        git("config", "user.email", "fixture@example.invalid")
        git("remote", "add", "origin", str(origin))
        fixture(checkout)
        git("add", ".")
        git("commit", "-q", "-m", "Canary main 0.1.0")
        git("branch", "release/0.1")
        git("push", "-q", "origin", "main", "release/0.1")
        environment = dict(GITHUB_ACTIONS="true", RUNNER_ENVIRONMENT="github-hosted", GITHUB_REPOSITORY="fixture/repository")
        create = dict(ref_type="branch", ref="release/0.1")
        module.advance(create, "create", checkout, environment)
        self.assertEqual(json.loads(git("show", "refs/remotes/origin/main:release-version.json"))["version"], "0.2.0")
        self.assertEqual(json.loads(git("show", "refs/remotes/origin/release/0.1:release-version.json"))["releaseType"], "Beta")
        main_commit = git("rev-parse", "refs/remotes/origin/main")
        module.advance(create, "create", checkout, environment)
        self.assertEqual(main_commit, git("rev-parse", "refs/remotes/origin/main"))
        git("tag", "v0.1.0", "refs/remotes/origin/release/0.1")
        git("push", "-q", "origin", "refs/tags/v0.1.0")
        source = git("rev-parse", "refs/tags/v0.1.0")
        payload = json.dumps(dict(Version="0.1.0", Channel="Release", ReleaseType="Beta", SourceBranch="release/0.1", Commit=source)).encode()
        event = dict(release=dict(tag_name="v0.1.0", draft=False, prerelease=True,
            assets=[dict(name="release-manifest.json", id=1, digest="sha256:" + hashlib.sha256(payload).hexdigest())]))
        def api(url, **kwargs):
            self.assertEqual(url, "https://api.github.com/repos/fixture/repository/releases/assets/1")
            self.assertTrue(kwargs["binary"])
            return payload
        module.advance(event, "release", checkout, environment, api=api)
        beta = git("rev-parse", "refs/remotes/origin/release/0.1")
        module.advance(event, "release", checkout, environment, api=api)
        self.assertEqual(beta, git("rev-parse", "refs/remotes/origin/release/0.1"))
        event["release"]["prerelease"] = False
        module.advance(event, "release", checkout, environment, api=api)
        promoted = json.loads(git("show", "refs/remotes/origin/release/0.1:release-version.json"))
        self.assertEqual((promoted["version"], promoted["releaseType"]), ("0.1.1", "Stable"))
        stable = git("rev-parse", "refs/remotes/origin/release/0.1")
        event["release"]["prerelease"] = True
        module.advance(event, "release", checkout, environment, api=api)
        self.assertEqual(stable, git("rev-parse", "refs/remotes/origin/release/0.1"))

        def publish(branch, version, prerelease):
            tag = "v" + version
            if not git("tag", "--list", tag):
                git("tag", tag, "refs/remotes/origin/" + branch)
                git("push", "-q", "origin", "refs/tags/" + tag)
            recorded = json.loads(git("show", f"refs/tags/{tag}:release-version.json"))
            data = json.dumps(dict(Version=version, Channel=recorded["channel"], ReleaseType=recorded["releaseType"],
                                   SourceBranch=branch, Commit=git("rev-parse", "refs/tags/" + tag))).encode()
            publication = dict(release=dict(tag_name=tag, draft=False, prerelease=prerelease,
                assets=[dict(name="release-manifest.json", id=1, digest="sha256:" + hashlib.sha256(data).hexdigest())]))
            module.advance(publication, "release", checkout, environment, api=lambda url, **kwargs: data)

        publish("main", "0.2.0", True)
        canary = git("rev-parse", "refs/remotes/origin/main")
        publish("main", "0.2.0", True)
        self.assertEqual(canary, git("rev-parse", "refs/remotes/origin/main"))
        publish("main", "0.2.1", True)
        self.assertEqual(json.loads(git("show", "refs/remotes/origin/main:release-version.json"))["version"], "0.2.2")
        git("branch", "release/0.2", "refs/remotes/origin/main")
        git("push", "-q", "origin", "release/0.2")
        module.advance(dict(ref_type="branch", ref="release/0.2"), "create", checkout, environment)
        self.assertEqual(json.loads(git("show", "refs/remotes/origin/release/0.2:release-version.json"))["releaseType"], "Beta")
        publish("release/0.1", "0.1.1", False)
        self.assertEqual(json.loads(git("show", "refs/remotes/origin/release/0.1:release-version.json"))["version"], "0.1.2")
        self.assertEqual(json.loads(git("show", "refs/remotes/origin/main:release-version.json"))["version"], "0.3.0")
        git("push", "-q", "origin", "--delete", "release/0.1")
        publish("release/0.1", "0.1.1", False)
        self.assertEqual(json.loads(git("show", "refs/remotes/origin/main:release-version.json"))["version"], "0.3.0")


if __name__ == "__main__":
    unittest.main()
