"""Behavior checks for portable command execution and graph boundaries."""
import importlib.util
import ntpath
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import repo_tools


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class CommandTests(unittest.TestCase):
    def test_dev_environment_forwards_child_separator_and_flags_literally(self):
        module = load("dev-env")
        child = ["cargo", "clippy", "--all-targets", "--", "-D", "warnings"]
        with patch.object(sys, "argv", ["dev-env.py", "--run", *child]), \
             patch.object(module, "development_environment", return_value={}), \
             patch.object(module, "command", return_value=subprocess.CompletedProcess(child, 0)) as run:
            with self.assertRaises(SystemExit) as completed:
                module.main()
        self.assertEqual(completed.exception.code, 0)
        run.assert_called_once_with(child, env={}, check=False)

    def test_arguments_are_literal_and_nonzero_exit_stops_the_caller(self):
        text = 'spaces & $() `echo` "quotes" ; Unicode: \u00e9'
        # Match command()'s UTF-8 decoder even on Windows without PYTHONUTF8.
        result = repo_tools.command([sys.executable, "-X", "utf8", "-c", "import sys; print(sys.argv[1])", text], capture=True)
        self.assertEqual(result.stdout.strip(), text)
        with self.assertRaises(subprocess.CalledProcessError):
            repo_tools.command([sys.executable, "-c", "raise SystemExit(19)"], capture=True)

    def test_native_environment_is_scoped_and_preserves_explicit_unix_compilers(self):
        environment = {"PATH": "/tools", "CC": "custom-clang", "CXX": "custom-clang++", "AR": "custom-ar"}
        with patch.object(repo_tools, "executable", side_effect=lambda name, env: "/tools/" + name):
            configured = repo_tools.development_environment(root=Path("/fixture"), environment=environment, platform="linux")
        self.assertEqual(environment, {"PATH": "/tools", "CC": "custom-clang", "CXX": "custom-clang++", "AR": "custom-ar"})
        self.assertEqual(configured["CXX"], "custom-clang++")
        self.assertEqual(configured["TRANSMOG_PYTHON"], sys.executable)

    def test_windows_environment_repairs_duplicate_paths_and_repeated_setup_is_stable(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            tool = root / ".tools/llvm/bin"
            tool.mkdir(parents=True)
            inherited = ";".join([str(tool).upper().replace("\\", "/") + "/"] * 40 +
                                 [r"C:\Node", r"C:\Other\bin", r"c:/node/", "", ""])
            environment = {"PATH": inherited, "ProgramFiles(x86)": str(root / "absent")}
            with patch.object(repo_tools, "executable", side_effect=lambda name, env: str(root / name)):
                configured = repo_tools.development_environment(root=root, environment=environment, platform="win32")
                original = configured["PATH"]
                for _ in range(32):
                    configured = repo_tools.development_environment(root=root, environment=configured, platform="win32")
            self.assertEqual(environment["PATH"], inherited)
            self.assertEqual(configured["PATH"], original)
            entries = original.split(";")
            normalized = [ntpath.normcase(ntpath.normpath(entry)) if entry else "" for entry in entries]
            self.assertEqual(len(normalized), len(set(normalized)))
            self.assertEqual(entries[0], str(tool))
            self.assertLess(entries.index(r"C:\Node"), entries.index(r"C:\Other\bin"))
            self.assertIn("", entries)

    @unittest.skipUnless(os.name == "nt", "Windows child-shell PATH handling")
    def test_windows_child_shell_can_resolve_tools_after_repairing_oversized_path(self):
        environment = dict(os.environ)
        environment["PATH"] = (str(Path(sys.executable).parent) + ";") * 160 + environment.get("PATH", "")
        self.assertGreater(len(environment["PATH"]), 8191)
        configured = repo_tools.development_environment(environment=environment)
        command = str(Path(os.environ["SystemRoot"]) / "System32/cmd.exe")
        result = subprocess.run([command, "/d", "/c", "where", Path(sys.executable).name],
                                env=configured, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(str(Path(sys.executable)), result.stdout)

    def test_every_portable_cli_can_show_help_without_build_tools(self):
        for name in ("dev-env", "build-desktop", "build-dotnet-sample", "test", "test-live", "test-load", "test-interop",
                     "test-linux-docker", "fuzz-content-docker", "update-content-fuzz-corpus-docker", "generate-supply-chain-artifacts",
                     "set-release-version", "check-published-release-version", "sync-stable-update-release", "sync-canary-update-feed", "advance-release-version",
                     "create-update-manifest", "cache-v8", "test-saz-interop", "benchmark-content", "setup-updater-signing", "trim-build-cache", "release-policy",
                     "update-chromium-psl", "vendor-boringssl-pki", "roll-chromium-dependencies", "compare-chromium"):
            with self.subTest(name=name):
                result = subprocess.run([sys.executable, SCRIPTS / (name + ".py"), "--help"], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)


class CryptoGraphTests(unittest.TestCase):
    def graph(self, separate_root=False):
        names = ["transmog-desktop", "tauri-plugin-updater", "rustls", "boring", "boring-sys"]
        packages = [{"id": name, "name": name, "version": "1"} for name in names]
        def edge(name, kind=None):
            return {"pkg": name, "dep_kinds": [{"kind": kind}]}
        nodes = [{"id": "transmog-desktop", "deps": [edge("tauri-plugin-updater"), edge("boring"), edge("boring-sys")]},
                 {"id": "tauri-plugin-updater", "deps": [edge("rustls")]},
                 *[{"id": name, "deps": []} for name in names[2:]]]
        return {"packages": packages, "resolve": {"nodes": nodes},
                "workspace_members": ["transmog-desktop", "rustls"] if separate_root else ["transmog-desktop"]}

    def test_exception_applies_only_to_desktop_updater_edge(self):
        graph = load("check-crypto-graph")
        graph.check_graph(self.graph(), "fixture")
        with self.assertRaisesRegex(RuntimeError, "Forbidden production TLS"):
            graph.check_graph(self.graph(separate_root=True), "fixture")

    def test_normal_edges_fail_and_dev_only_edges_are_excluded(self):
        checker = load("check-crypto-graph")
        graph = self.graph()
        graph["resolve"]["nodes"][0]["deps"].append({"pkg": "rustls", "dep_kinds": [{"kind": "dev"}]})
        checker.check_graph(graph, "fixture")
        graph["resolve"]["nodes"][0]["deps"][-1]["dep_kinds"] = [{"kind": None}]
        with self.assertRaisesRegex(RuntimeError, "Forbidden production TLS"):
            checker.check_graph(graph, "fixture")

    def test_legacy_hyper_connector_is_allowed_only_as_a_test_dependency(self):
        checker = load("check-crypto-graph")
        graph = self.graph()
        graph["packages"].append({"id": "hyper-boring", "name": "hyper-boring", "version": "5.2.0"})
        graph["resolve"]["nodes"].append({"id": "hyper-boring", "deps": []})
        edge = {"pkg": "hyper-boring", "dep_kinds": [{"kind": "dev"}]}
        graph["resolve"]["nodes"][0]["deps"].append(edge)
        checker.check_graph(graph, "fixture")
        edge["dep_kinds"] = [{"kind": None}]
        with self.assertRaisesRegex(RuntimeError, "Forbidden production TLS.*hyper-boring"):
            checker.check_graph(graph, "fixture")

    def test_same_version_tls_copies_are_distinct_only_in_the_production_graph(self):
        checker = load("check-crypto-graph")
        registry = "registry+https://github.com/rust-lang/crates.io-index"
        git = "git+https://github.com/cloudflare/boring?rev="
        sources = [(registry, git + "1" * 40), (git + "1" * 40, git + "2" * 40)]
        for name in ("boring", "boring-sys"):
            for first_source, second_source in sources:
                for kind in (None, "dev", "build"):
                    with self.subTest(name=name, sources=(first_source, second_source), kind=kind):
                        graph = self.graph()
                        original = next(p for p in graph["packages"] if p["name"] == name)
                        original.update(version="5.2.0", source=first_source)
                        first_id = f"{first_source}#{name}@5.2.0"
                        second_id = f"{second_source}#{name}@5.2.0"
                        original["id"] = first_id
                        for node in graph["resolve"]["nodes"]:
                            if node["id"] == name:
                                node["id"] = first_id
                            for dependency in node["deps"]:
                                if dependency["pkg"] == name:
                                    dependency["pkg"] = first_id
                        graph["packages"].append(dict(original, id=second_id, source=second_source))
                        graph["resolve"]["nodes"].append({"id": second_id, "deps": []})
                        graph["resolve"]["nodes"][0]["deps"].append({
                            "pkg": second_id, "dep_kinds": [{"kind": kind}]})
                        if kind is None:
                            with self.assertRaisesRegex(RuntimeError, f"Expected one {name} package") as error:
                                checker.check_graph(graph, "fixture")
                            self.assertIn(first_id, str(error.exception))
                            self.assertIn(second_id, str(error.exception))
                        else:
                            checker.check_graph(graph, "fixture")

    def test_desktop_client_exception_requires_the_updaters_exact_package(self):
        checker = load("check-crypto-graph")
        graph = self.graph()
        def edge(name):
            return {"pkg": name, "dep_kinds": [{"kind": None}]}
        graph["packages"].append({"id": "reqwest", "name": "reqwest", "version": "1"})
        graph["resolve"]["nodes"].append({"id": "reqwest", "deps": [edge("rustls")]})
        graph["resolve"]["nodes"][0]["deps"].append(edge("reqwest"))
        graph["resolve"]["nodes"][1]["deps"] = [edge("reqwest")]
        checker.check_graph(graph, "fixture")
        graph["workspace_members"].append("reqwest")
        with self.assertRaisesRegex(RuntimeError, "Forbidden production TLS"):
            checker.check_graph(graph, "fixture")
        graph["workspace_members"].pop()
        graph["packages"].append({"id": "reqwest-2", "name": "reqwest", "version": "2"})
        graph["resolve"]["nodes"].append({"id": "reqwest-2", "deps": [edge("rustls")]})
        graph["resolve"]["nodes"][0]["deps"].append(edge("reqwest-2"))
        with self.assertRaisesRegex(RuntimeError, "Forbidden production TLS"):
            checker.check_graph(graph, "fixture")
        graph["packages"].append({"id": "updater-2", "name": "tauri-plugin-updater", "version": "2"})
        graph["resolve"]["nodes"].append({"id": "updater-2", "deps": [edge("reqwest-2")]})
        with self.assertRaisesRegex(RuntimeError, "Forbidden production TLS"):
            checker.check_graph(graph, "fixture")


class SupplyChainTests(unittest.TestCase):
    def test_nested_scoped_production_packages_and_notice_order(self):
        module = load("generate-supply-chain-artifacts")
        packages = module.production_npm({"packages": {"": {"version": "1"},
            "node_modules/outer/node_modules/@scope/pkg": {"version": "2", "license": "MIT", "resolved": "https://example.invalid/pkg"},
            "node_modules/dev-only": {"version": "3", "dev": True}}})
        self.assertEqual([p["name"] for p in packages], ["@scope/pkg"])
        notice, sbom = module.artifacts({"packages": [], "resolve": {"nodes": []}}, packages, "1.2.3")
        self.assertIn("| @scope/pkg | 2 | MIT |", notice)
        component = next(item for item in sbom["components"] if item["bom-ref"] == "npm:@scope/pkg@2")
        self.assertEqual(component["purl"], "pkg:npm/%40scope/pkg@2")
        with self.assertRaisesRegex(ValueError, "Unexpected npm"):
            module.production_npm({"packages": {"not-a-package": {"version": "1"}}})


if __name__ == "__main__":
    unittest.main()
