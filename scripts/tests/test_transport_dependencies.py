import io
import json
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import transport_dependencies as transport


class TransportSnapshots(unittest.TestCase):
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
