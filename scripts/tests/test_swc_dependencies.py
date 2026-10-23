import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import swc_dependencies as swc


class SwcSnapshotTests(unittest.TestCase):
    def fixture(self, root):
        directory = root / swc.DIRECTORY
        directory.mkdir(parents=True)
        files = {"Cargo.toml": b'[package]\nname="swc_ecma_parser"\n',
                 "parser.patch": b"reviewed patch\n", "LICENSE": b"upstream license\n"}
        for name, data in files.items():
            (directory / name).write_bytes(data)
        receipt = {"source": {"version": swc.VERSION, "archiveSha256": swc.ARCHIVE_SHA256,
                              "archiveUrl": swc.ARCHIVE_URL},
                   "patchSha256": swc.digest(files["parser.patch"]),
                   "files": {name: swc.digest(data) for name, data in files.items()}}
        (directory / "transmog-snapshot.json").write_text(json.dumps(receipt))
        return directory

    def test_repository_snapshot_matches_its_receipt(self):
        swc.check_repository()

    def test_substituted_archive_identity_fails(self):
        for key in ("version", "archiveSha256", "archiveUrl"):
            with self.subTest(key=key), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                directory = self.fixture(root)
                receipt_path = directory / "transmog-snapshot.json"
                receipt = json.loads(receipt_path.read_text())
                receipt["source"][key] = "substituted"
                receipt_path.write_text(json.dumps(receipt))
                with self.assertRaises(ValueError):
                    swc.check_repository(root)

    def test_changed_missing_and_extra_files_fail_verification(self):
        for mutation in ("changed", "missing", "extra"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                directory = self.fixture(root)
                if mutation == "changed":
                    (directory / "LICENSE").write_text("changed")
                elif mutation == "missing":
                    (directory / "LICENSE").unlink()
                else:
                    (directory / "unexpected.rs").write_text("unreviewed")
                with self.assertRaises(ValueError):
                    swc.check_repository(root)

    def test_resolved_package_must_use_the_reviewed_path_and_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            directory = self.fixture(root)
            package = {"id": "parser", "name": "swc_ecma_parser", "version": swc.VERSION,
                       "source": None, "manifest_path": str(directory / "Cargo.toml")}
            provenance = swc.resolved_provenance({"packages": [package]}, root)
            self.assertEqual(provenance["parser"]["archiveSha256"], swc.ARCHIVE_SHA256)
            for replacement in ({"source": "registry+crates.io"}, {"version": "0.0.0"},
                                {"manifest_path": str(root / "Cargo.toml")}):
                with self.subTest(replacement=replacement), self.assertRaises(ValueError):
                    swc.check_repository(root, {"packages": [{**package, **replacement}]})


if __name__ == "__main__":
    unittest.main()
