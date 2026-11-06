"""Verify the local rusty_v8 binding fork and its native profile provenance."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path

from dependency_snapshots import snapshot_file
from repo_tools import ROOT

DIRECTORY = "third_party/rusty-v8"
VERSION = "152.2.0"
ARCHIVE_SHA256 = "a10fe1a92da5c32c7c7f838ce36c0ccfcfd5edf0865b58bdde820aa64cea9888"
NATIVE_REVISION = "c4ca1eccb90c5464d826b7713dc27178a53b0cfe"
ICU_REVISION = "d578f2e8b7bd5938e21cfb6bf15c079e0aa5b738"
ICU_DATA_SHA256 = "9f48c7f9c7c94d516a14870707e910ab94d75ae640ff6842c4af53276cd26ebe"


def check_repository(root=ROOT, metadata=None):
    directory = root / DIRECTORY
    pin = json.loads((directory / "source.json").read_text(encoding="utf-8"))
    if pin["version"] != VERSION or pin["archive_sha256"] != ARCHIVE_SHA256:
        raise ValueError("V8 must use the reviewed registry source archive")
    if (pin.get("icuData", {}).get("revision") != ICU_REVISION
            or pin["icuData"].get("sha256") != ICU_DATA_SHA256):
        raise ValueError("V8 must use the reviewed ICU data source")
    receipt_path = directory / "transmog-snapshot.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    if receipt.get("source") != pin or receipt.get("nativeRevision") != NATIVE_REVISION:
        raise ValueError("V8 snapshot does not identify the reviewed native source")
    actual = {}
    for file in directory.rglob("*"):
        name = file.relative_to(directory).as_posix()
        checked = snapshot_file(directory, name)
        if checked.is_file() and name != "transmog-snapshot.json":
            actual[name] = hashlib.sha256(checked.read_bytes()).hexdigest()
    if actual != receipt["files"]:
        raise ValueError("V8 binding/profile snapshot differs from its reviewed receipt")
    if metadata is not None:
        for package in metadata["packages"]:
            if package["name"] == "v8" and (
                package["version"] != VERSION or package.get("source") is not None
                or Path(package["manifest_path"]).resolve() != (directory / "Cargo.toml").resolve()
            ):
                raise ValueError("Resolved V8 does not use the reviewed binding/native profile")
    return receipt


def resolved_provenance(metadata, root=ROOT):
    packages = [p for p in metadata["packages"] if p["name"] == "v8"]
    if not packages:
        return {}
    check_repository(root, metadata)
    directory = root / DIRECTORY
    return {p["id"]: {
        "version": VERSION,
        "archiveSha256": ARCHIVE_SHA256,
        "archiveUrl": f"https://static.crates.io/crates/v8/v8-{VERSION}.crate",
        "patchPath": DIRECTORY + "/bindings.patch",
        "patchSha256": hashlib.sha256((directory / "bindings.patch").read_bytes()).hexdigest(),
        "snapshotSha256": hashlib.sha256((directory / "transmog-snapshot.json").read_bytes()).hexdigest(),
        "propertyPrefix": "transmog:v8:",
    } for p in packages}
