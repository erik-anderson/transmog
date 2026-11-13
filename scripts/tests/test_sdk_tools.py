"""Regression checks for the opt-in SDK source bootstrap."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import sdk_tools
from repo_tools import command


class SelectionTests(unittest.TestCase):
    def test_sdk_overrides_match_the_repository(self):
        sdk_tools.verify_selections()


class CompilerArgumentsTests(unittest.TestCase):
    def test_receipts_supply_per_unit_dependency_paths(self):
        paths = ["target/debug/build/library/789/out/liblibrary.rlib", "target/debug/build/new/123/out/libnew.rmeta",
                 "target/debug/build/macros/456/out/macros.dll"]
        receipts = [{"reason": "compiler-artifact", "filenames": paths + [paths[0], "target/debug/bin.exe"]},
                    {"reason": "build-script-executed", "filenames": ["unrelated/libfake.rlib"]}]
        arguments = sdk_tools.dependency_search_arguments(receipts)
        self.assertEqual(arguments[::2], ["-L"] * 3)
        self.assertEqual(set(arguments[1::2]), {f"dependency={Path(path).parent}" for path in paths})

    def test_response_file_preserves_spaces_and_avoids_shell_quoting(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "compiler args.rsp"
            values = ["--h-out", "a directory/header.hpp", "-L", "dependency=C:/some path/out"]
            self.assertEqual(sdk_tools.write_response_file(path, values), "@" + str(path))
            self.assertEqual(path.read_text().splitlines(), values)
            with self.assertRaises(ValueError):
                sdk_tools.write_response_file(path, ["invalid\nargument"])


class GeneratorEnvironmentTests(unittest.TestCase):
    @unittest.skipIf(os.name == "nt", "Linux compiler-private linkage")
    def test_llvm_linkage_and_runtime_paths_are_scoped_to_the_generator(self):
        compiler = Path("/compiler with spaces")
        for flags in ({"RUSTFLAGS": "-C debuginfo=1"},
                      {"CARGO_ENCODED_RUSTFLAGS": "-C\x1fdebuginfo=1"}):
            with self.subTest(flags=flags):
                original = {"PATH": "/usr/bin", "LD_LIBRARY_PATH": "/existing", **flags}
                generator = sdk_tools.generator_environment(original, compiler)
                self.assertEqual(generator["CARGO_ENCODED_RUSTFLAGS"].split("\x1f"),
                                 ["-C", "debuginfo=1", "-L", "native=/compiler with spaces/lib"])
                self.assertEqual(generator["LD_LIBRARY_PATH"], "/compiler with spaces/lib:/existing")
                self.assertEqual(generator["RUSTC_BOOTSTRAP"], "1")
                self.assertEqual(original, {"PATH": "/usr/bin", "LD_LIBRARY_PATH": "/existing", **flags})


class CrubitCheckoutTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.upstream = self.root / "upstream"
        command(["git", "init", self.upstream], capture=True)
        (self.upstream / "input.txt").write_text("pinned input\n", encoding="utf-8")
        command(["git", "-C", self.upstream, "add", "input.txt"], capture=True)
        command(["git", "-C", self.upstream, "-c", "user.name=SDK test",
                 "-c", "user.email=sdk-test@example.invalid", "-c", "commit.gpgsign=false",
                 "commit", "-m", "fixture"], capture=True)
        self.revision = command(["git", "-C", self.upstream, "rev-parse", "HEAD"], capture=True).stdout.strip()
        self.source = self.root / "checkout"
        self.pin = patch.dict(sdk_tools.PIN, crubit_repository=str(self.upstream), crubit_revision=self.revision)
        self.pin.start()
        self.addCleanup(self.pin.stop)

    def test_retries_after_fetch_failure(self):
        with patch.dict(sdk_tools.PIN, crubit_repository=str(self.root / "unavailable")):
            with self.assertRaises(subprocess.CalledProcessError):
                sdk_tools.ensure_crubit_source(self.source, os.environ)
        # The failed fetch already initialized .git, but must not strand retries.
        sdk_tools.ensure_crubit_source(self.source, os.environ)
        sdk_tools.ensure_crubit_source(self.source, os.environ)
        self.assertEqual((self.source / "input.txt").read_text(encoding="utf-8"), "pinned input\n")

    def test_does_not_reset_modified_or_different_checkouts(self):
        sdk_tools.ensure_crubit_source(self.source, os.environ)
        input_file = self.source / "input.txt"
        input_file.write_text("local edit\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "must be clean"):
            sdk_tools.ensure_crubit_source(self.source, os.environ)
        self.assertEqual(input_file.read_text(encoding="utf-8"), "local edit\n")
        input_file.write_text("pinned input\n", encoding="utf-8")
        with patch.dict(sdk_tools.PIN, crubit_revision="0" * 40):
            with self.assertRaisesRegex(ValueError, "Expected Crubit"):
                sdk_tools.ensure_crubit_source(self.source, os.environ)
        self.assertEqual(command(["git", "-C", self.source, "rev-parse", "HEAD"], capture=True).stdout.strip(),
                         self.revision)


if __name__ == "__main__":
    unittest.main()
