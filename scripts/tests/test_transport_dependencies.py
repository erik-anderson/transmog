import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import transport_dependencies as transport


class TransportSnapshots(unittest.TestCase):
    def test_receipted_transport_files_are_tracked(self):
        tracked = subprocess.run(
            ["git", "ls-files", "-z", "--", "third_party/rust-transport/vendor"],
            cwd=transport.ROOT, capture_output=True, check=True,
        ).stdout.decode("utf-8").split("\0")
        tracked = set(tracked)
        for name in ("h2", "hyper", "quiche"):
            directory = transport.BASE / "vendor" / name
            receipt = json.loads((directory / "transmog-snapshot.json").read_text(encoding="utf-8"))
            prefix = directory.relative_to(transport.ROOT).as_posix() + "/"
            missing = {prefix + filename for filename in receipt["files"]} - tracked
            self.assertFalse(missing, f"Snapshot files missing from Git: {sorted(missing)}")

    def test_regeneration_ignores_git_user_line_ending_preferences(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(transport, "BASE", Path(temporary)):
            root = Path(temporary)
            (root / "patches").mkdir()
            data = self.archive("h2-1.0.0/file")
            (root / "h2-1.0.0.crate").write_bytes(data)
            (root / "patches/h2.patch").write_bytes(b"--- a/file\n+++ b/file\n@@ -1 +1 @@\n-x\n+y\n")
            # Give the archive's text file a final newline for the patch.
            output = io.BytesIO()
            with tarfile.open(fileobj=output, mode="w:gz") as archive:
                member = tarfile.TarInfo("h2-1.0.0/file")
                member.size = 2
                archive.addfile(member, io.BytesIO(b"x\n"))
            data = output.getvalue()
            (root / "h2-1.0.0.crate").write_bytes(data)
            sources = {"h2": {"version": "1.0.0", "archiveSha256": transport.digest(data)}}
            with patch.dict(os.environ, {"GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "core.autocrlf", "GIT_CONFIG_VALUE_0": "true"}):
                transport.regenerate(sources, root)
            self.assertEqual((root / "vendor/h2/file").read_bytes(), b"y\n")
            transport.check(sources)
            stamp = (root / "vendor/h2/file").stat().st_mtime_ns
            transport.regenerate(sources, root)
            self.assertEqual((root / "vendor/h2/file").stat().st_mtime_ns, stamp)

    def archive(self, name, *, kind=tarfile.REGTYPE):
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w:gz") as archive:
            member = tarfile.TarInfo(name)
            member.type = kind
            member.size = 1 if kind == tarfile.REGTYPE else 0
            archive.addfile(member, io.BytesIO(b"x"))
        return output.getvalue()

    def test_archive_rejects_escaping_aliased_and_linked_paths(self):
        with tempfile.TemporaryDirectory() as temporary:
            for name in ("outside/file", "h2-1/../escape", "h2-1/C:/file", "h2-1/dir\\file"):
                with self.subTest(name=name), self.assertRaises(ValueError):
                    transport.unpack(self.archive(name), Path(temporary), "h2-1")
            with self.assertRaises(ValueError):
                transport.unpack(self.archive("h2-1/link", kind=tarfile.SYMTYPE), Path(temporary), "h2-1")

    def test_receipt_rejects_changes_additions_removals_and_patch_edits(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(transport, "BASE", Path(temporary)):
            root = Path(temporary)
            (root / "patches").mkdir()
            (root / "patches/h2.patch").write_bytes(b"")
            directory = root / "vendor/h2"
            directory.mkdir(parents=True)
            (directory / "LICENSE").write_bytes(b"upstream")
            source = {"version": "1.0.0", "archiveSha256": "a" * 64}
            receipt = {"source": source, "patchSha256": transport.digest(b""), "files": transport.inventory(directory)}
            (directory / "transmog-snapshot.json").write_text(json.dumps(receipt))
            transport.check({"h2": source})
            for filename, content in (("LICENSE", b"changed"), ("extra", b"extra")):
                path = directory / filename
                path.write_bytes(content)
                with self.assertRaises(ValueError):
                    transport.check({"h2": source})
                path.unlink()
                (directory / "LICENSE").write_bytes(b"upstream")
            (directory / "LICENSE").unlink()
            with self.assertRaises(ValueError):
                transport.check({"h2": source})
            (directory / "LICENSE").write_bytes(b"upstream")
            (root / "patches/h2.patch").write_bytes(b"changed")
            with self.assertRaises(ValueError):
                transport.check({"h2": source})

    def test_exact_registry_versions_and_checksums_are_required(self):
        valid = {name: {"version": "1.0.0", "archiveSha256": "a" * 64} for name in ("h2", "hyper", "quiche")}
        transport.validate_sources(valid)
        for version in ("latest", "../escape", "^1.0.0"):
            with self.assertRaises(ValueError):
                transport.validate_sources({**valid, "h2": {**valid["h2"], "version": version}})


if __name__ == "__main__":
    unittest.main()
