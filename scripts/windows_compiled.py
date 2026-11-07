"""Transfer a source-bound x64-hosted Windows build to native qualification."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import zipfile
import hashlib
import stat

from repo_tools import ROOT, main_guard
from release_version import State
from v8_artifact import digest, encoded_digest

BINARIES = ("transmog.exe", "transmog-cli.exe", "transmog-script-host.exe", "transmog-preview-worker.exe")
PDBS = tuple(Path(name).stem.replace("-", "_") + ".pdb" for name in BINARIES)
TARGETS = {"x86_64-pc-windows-msvc": 0x8664, "aarch64-pc-windows-msvc": 0xAA64}


def catalog(directory):
    result = {}
    for path in directory.rglob("*"):
        if path.is_symlink() or getattr(path.lstat(), "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT:
            raise ValueError("Compiled build links are forbidden")
        if path.is_file() and path != directory / "manifest.json":
            result[path.relative_to(directory).as_posix()] = digest(path)
    return result


def binary_target(path, target):
    with path.open("rb") as stream:
        length = path.stat().st_size
        if length < 64 or stream.read(2) != b"MZ":
            raise ValueError("Compiled output has no executable header")
        stream.seek(60)
        offset = struct.unpack("<I", stream.read(4))[0]
        if offset > length - 6:
            raise ValueError("Compiled executable has an invalid PE offset")
        stream.seek(offset)
        if stream.read(4) != b"PE\0\0" or struct.unpack("<H", stream.read(2))[0] != TARGETS[target]:
            raise ValueError("Compiled executable architecture differs from its target")


def verify_manifest(manifest, target, commit, run_id, attempt):
    expected = {"Schema": 1, "Commit": commit, "RunId": run_id, "Target": target,
                "Host": "x86_64-pc-windows-msvc"}
    if any(manifest.get(name) != value for name, value in expected.items()):
        raise ValueError("Compiled build source, run, host or target differs")
    producer_attempt = manifest.get("RunAttempt", "")
    if not str(producer_attempt).isdigit() or not 1 <= int(producer_attempt) <= int(attempt):
        raise ValueError("Compiled build attempt is invalid or from the future")
    gate = manifest["Gate"]
    expected_gate = {"Name": "Build", "Result": "passed", "Stage": "Package", "Commit": commit,
                     "RunId": run_id, "RunAttempt": producer_attempt, "Target": target}
    if any(gate.get(name) != value for name, value in expected_gate.items()):
        raise ValueError("Compiled build did not pass its owning build gate")
    names = set(manifest["Files"])
    required = {"binaries/" + name for name in (*BINARIES, *PDBS)}
    required |= {"evidence/THIRD_PARTY_NOTICES.md", "evidence/sbom.cdx.json"}
    if not required <= names or not any(name.startswith("ui/") for name in names):
        raise ValueError("Compiled build is missing binaries, symbols, notices or UI")
    if any(name not in required and not name.startswith("ui/") for name in names):
        raise ValueError("Unexpected compiled build file")
    for name, checksum in manifest["Files"].items():
        if ("\\" in name or name.startswith("/") or any(part in {"", ".", ".."} for part in name.split("/"))
                or ":" in name or len(checksum) != 64 or any(char not in "0123456789abcdef" for char in checksum)):
            raise ValueError("Invalid compiled build path or checksum")
    build = manifest["Build"]
    State.from_dict(build)
    if build["Target"] != target or build["BuildDirectory"] != "binaries":
        raise ValueError("Compiled build metadata differs")
    engine = manifest["Engine"]
    if (not str(engine.get("ArtifactId", "")).isdigit()
            or engine["Manifest"]["identity"]["target"] != target
            or engine["Manifest"]["key"] != encoded_digest(engine["Manifest"]["identity"])):
        raise ValueError("Compiled build engine provenance differs")


def export(directory, target, *, root=ROOT):
    if directory.exists():
        raise ValueError("Compiled export requires a fresh directory")
    state = json.loads((root / "artifacts/windows-build-state.json").read_text(encoding="utf-8-sig"))
    host = subprocess.run(["rustc", "-vV"], check=True, text=True, capture_output=True).stdout
    if "host: x86_64-pc-windows-msvc" not in host.splitlines():
        raise ValueError("Production compilation must use the native x64 Rust host")
    engine = Path(os.environ["TRANSMOG_V8_ARTIFACT"])
    manifest = {"Schema": 1, "Host": "x86_64-pc-windows-msvc", "Commit": state["Commit"],
                "RunId": state["RunId"], "RunAttempt": state["RunAttempt"], "Target": target,
                "Gate": state["Phases"]["Build"], "Build": dict(state["Build"], BuildDirectory="binaries"),
                "Engine": {"ArtifactId": os.environ["V8_ARTIFACT_ID"],
                           "Manifest": json.loads((engine / "manifest.json").read_text(encoding="utf-8")),
                           "ManifestSha256": digest(engine / "manifest.json")}, "Files": {}}
    (directory / "binaries").mkdir(parents=True)
    for name in (*BINARIES, *PDBS):
        shutil.copyfile(Path(state["Build"]["BuildDirectory"]) / name, directory / "binaries" / name)
    shutil.copytree(root / "apps/desktop/ui/dist", directory / "ui")
    (directory / "evidence").mkdir()
    for name in ("THIRD_PARTY_NOTICES.md", "sbom.cdx.json"):
        shutil.copyfile(root / "artifacts/windows-build/evidence" / name, directory / "evidence" / name)
    manifest["Files"] = catalog(directory)
    verify_manifest(manifest, target, os.environ["GITHUB_SHA"], os.environ["GITHUB_RUN_ID"], os.environ["GITHUB_RUN_ATTEMPT"])
    for name in BINARIES:
        binary_target(directory / "binaries" / name, target)
    (directory / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


def verify_evidence(evidence, target, commit, run_id, attempt):
    if not str(evidence.get("ArtifactId", "")).isdigit():
        raise ValueError("Compiled build requires an immutable artifact ID")
    manifest = evidence["Manifest"]
    if evidence["ManifestCanonicalSha256"] != encoded_digest(manifest):
        raise ValueError("Compiled build manifest digest differs")
    verify_manifest(manifest, target, commit, run_id, attempt)
    return manifest


def verify_qualified(directory, target, commit, run_id, attempt):
    evidence = json.loads((directory / "evidence/compiled-build.json").read_text(encoding="utf-8"))
    producer = verify_evidence(evidence, target, commit, run_id, attempt)
    engine = json.loads((directory / "evidence/v8-engine-package.json").read_text(encoding="utf-8-sig"))
    if (producer["Engine"]["ArtifactId"] != engine["ArtifactId"]
            or producer["Engine"]["Manifest"] != engine["Engine"]
            or producer["Engine"]["ManifestSha256"].lower() != engine["ManifestSha256"].lower()):
        raise ValueError("Compiled package and qualification engines differ")
    for name, checksum in producer["Files"].items():
        if name.startswith("binaries/"):
            if not name.endswith(".exe"):
                continue
            relative = "target/release/" + name.removeprefix("binaries/")
        elif name.startswith("ui/"):
            relative = "apps/desktop/ui/dist/" + name.removeprefix("ui/")
        else:
            relative = name
        if digest(directory / relative) != checksum:
            raise ValueError("Native qualification changed a compiled payload file")
    architecture = "arm64" if target == "aarch64-pc-windows-msvc" else "x64"
    symbols = directory / f'evidence/Transmog_{producer["Build"]["Version"]}_windows-{architecture}-symbols.zip'
    with zipfile.ZipFile(symbols) as archive:
        for name in PDBS:
            if archive.namelist().count(name) != 1:
                raise ValueError("Missing or duplicate original compiled symbols")
            with archive.open(name) as stream:
                if hashlib.file_digest(stream, "sha256").hexdigest() != producer["Files"]["binaries/" + name]:
                    raise ValueError("Native qualification changed compiled symbols")


def restore(directory, target, artifact_id, *, root=ROOT):
    manifest = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))
    evidence = {"ArtifactId": artifact_id, "ManifestCanonicalSha256": encoded_digest(manifest), "Manifest": manifest}
    verify_evidence(evidence, target, os.environ["GITHUB_SHA"], os.environ["GITHUB_RUN_ID"], os.environ["GITHUB_RUN_ATTEMPT"])
    if manifest["Files"] != catalog(directory):
        raise ValueError("Compiled build checksum catalog differs")
    engine = Path(os.environ["TRANSMOG_V8_ARTIFACT"])
    if (manifest["Engine"]["ArtifactId"] != os.environ["V8_ARTIFACT_ID"]
            or manifest["Engine"]["ManifestSha256"] != digest(engine / "manifest.json")
            or manifest["Engine"]["Manifest"] != json.loads((engine / "manifest.json").read_text(encoding="utf-8"))):
        raise ValueError("Compiled build and native qualification use different engines")
    for name in BINARIES:
        binary_target(directory / "binaries" / name, target)
    ui = root / "apps/desktop/ui/dist"
    if ui.exists():
        raise ValueError("Native qualification requires a fresh UI destination")
    shutil.copytree(directory / "ui", ui)
    evidence_directory = root / "artifacts/windows-build/evidence"
    for source in (directory / "evidence").iterdir():
        shutil.copyfile(source, evidence_directory / source.name)
    evidence["VerifiedAtUtc"] = datetime.now(timezone.utc).isoformat()
    (evidence_directory / "compiled-build.json").write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
    return dict(manifest["Build"], BuildDirectory=str((directory / "binaries").resolve()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("export", "restore", "verify"))
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--artifact-id")
    args = parser.parse_args()
    if args.operation == "export":
        export(args.directory, args.target)
    elif args.operation == "verify":
        pipeline = json.loads((args.directory / "evidence/pipeline-inputs.json").read_text(encoding="utf-8"))
        verify_qualified(args.directory, args.target, pipeline["Commit"], pipeline["RunId"], pipeline["RunAttempt"])
    else:
        print(json.dumps(restore(args.directory, args.target, args.artifact_id)))


if __name__ == "__main__":
    main_guard(main)
