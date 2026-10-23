"""Verify the narrowly patched SWC parser and expose its distribution provenance."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path

from dependency_snapshots import snapshot_file
from repo_tools import ROOT, main_guard

VERSION = "27.0.7"
ARCHIVE_SHA256 = "7f1a51af1a92cd4904c073b293e491bbc0918400a45d58227b34c961dd6f52d7"
DIRECTORY = "third_party/swc-ecma-parser"
ARCHIVE_URL = f"https://static.crates.io/crates/swc_ecma_parser/swc_ecma_parser-{VERSION}.crate"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def check_repository(root=ROOT, metadata=None):
    directory = root / DIRECTORY
    receipt_path = directory / "transmog-snapshot.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    if receipt["source"] != {"version": VERSION, "archiveSha256": ARCHIVE_SHA256,
                             "archiveUrl": ARCHIVE_URL}:
        raise ValueError("SWC parser must use the reviewed registry archive")
    patch = directory / "parser.patch"
    if digest(patch.read_bytes()) != receipt["patchSha256"]:
        raise ValueError("SWC parser patch differs from its reviewed receipt")
    actual = {}
    for file in directory.rglob("*"):
        name = file.relative_to(directory).as_posix()
        checked = snapshot_file(directory, name)
        if checked.is_file() and name != "transmog-snapshot.json":
            actual[name] = digest(checked.read_bytes())
    if actual != receipt["files"]:
        raise ValueError("SWC parser snapshot inventory differs from its reviewed receipt")
    if metadata is not None:
        for package in metadata["packages"]:
            if package["name"] == "swc_ecma_parser" and (
                package["version"] != VERSION or package.get("source") is not None
                or Path(package["manifest_path"]).resolve() != (directory / "Cargo.toml").resolve()
            ):
                raise ValueError("Resolved SWC parser does not use the reviewed snapshot")
    return receipt


def resolved_provenance(metadata, root=ROOT):
    packages = [package for package in metadata["packages"] if package["name"] == "swc_ecma_parser"]
    if not packages:
        return {}
    receipt = check_repository(root, metadata)
    return {package["id"]: {
        **receipt["source"],
        "patchPath": DIRECTORY + "/parser.patch",
        "patchSha256": receipt["patchSha256"],
        "snapshotSha256": digest((root / DIRECTORY / "transmog-snapshot.json").read_bytes()),
        "propertyPrefix": "transmog:swc:",
    } for package in packages}


def main():
    check_repository()
    print("SWC parser dependency snapshot verified")


if __name__ == "__main__":
    main_guard(main)
