"""Copy/check an unmodified PKI target against the pinned boring-sys submodule.

Generation uses a fresh staging directory and removes obsolete upstream files.
No SSL or crypto implementation is built here. PKI links to boring-sys's crypto.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib

from dependency_snapshots import replace_directory, snapshot_file

ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / "third_party" / "boringssl-pki"
FAMILY = ("boring", "boring-sys", "hyper-boring", "tokio-boring")


def check_snapshot(destination: Path, *, revision=None, boring_revision=None):
    manifest = json.loads((destination / "manifest.json").read_text(encoding="utf-8"))
    for key, expected in (("revision", revision), ("boring_revision", boring_revision)):
        if not re.fullmatch(r"[0-9a-f]{40}", manifest[key]) or expected is not None and manifest[key] != expected:
            raise ValueError(f"PKI receipt has an unexpected {key}")
    files = manifest["files"]
    sources = manifest["sources"]
    if not sources or len(sources) != len(set(sources)):
        raise ValueError("Missing or duplicate PKI sources")
    for name in sources:
        if name not in files or not name.startswith("pki/") or not name.endswith(".cc"):
            raise ValueError(f"Unexpected PKI source: {name}")
    for name, digest in files.items():
        path = snapshot_file(destination, name)
        if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError(f"PKI source hash mismatch: {name}")
    actual = set()
    for path in destination.rglob("*"):
        if path.is_symlink() or path.resolve() != path.absolute():
            raise ValueError(f"Linked file in PKI snapshot: {path}")
        if path.is_file():
            actual.add(path.relative_to(destination).as_posix())
    expected = set(files) | {"manifest.json", "README.md"}
    if actual != expected:
        raise ValueError(f"PKI snapshot file set differs: extra={sorted(actual - expected)}, missing={sorted(expected - actual)}")
    return manifest


def generate_snapshot(source: Path, destination: Path, revision: str, boring_revision: str, boring_version=None):
    source = source.resolve(strict=True)
    for value in (revision, boring_revision):
        if not re.fullmatch(r"[0-9a-f]{40}", value):
            raise ValueError("Use immutable full Git revisions")
    actual = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    if actual != revision:
        raise ValueError(f"Expected BoringSSL {revision}, got {actual}")
    status = subprocess.check_output(["git", "-C", str(source), "status", "--porcelain", "--untracked-files=all"], text=True)
    # Cargo adds this cache marker even inside a fetched submodule. It is not
    # source code and is never included in the snapshot.
    if any(line != "?? .cargo-ok" for line in status.splitlines()):
        raise ValueError("BoringSSL source checkout must be clean")
    cmake = (source / "gen/sources.cmake").read_text(encoding="utf-8")
    block = re.search(r"set\(\s*PKI_SOURCES\s+(.*?)\n\)", cmake, re.S)
    if block is None:
        raise ValueError("Upstream PKI source target is missing")
    sources = block.group(1).split()
    pending = [snapshot_file(source, name) for name in sources]
    pending.extend((source / "include").rglob("*.h"))
    pending.extend((source / "pki").glob("*.h"))
    pending.append(source / "LICENSE")
    destination = destination.absolute()
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".pki-stage-", dir=destination.parent) as temporary:
        staged = Path(temporary)
        files = {}
        while pending:
            path = pending.pop()
            name = path.relative_to(source).as_posix()
            snapshot_file(source, name)
            if name in files:
                continue
            data = path.read_bytes()
            target = staged / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            files[name] = hashlib.sha256(data).hexdigest()
            for include in re.findall(rb'^\s*#\s*include\s*[<"]([^>"]+)[>"]', data, re.M):
                include_path = include.decode()
                for candidate in (path.parent / include_path, source / "include" / include_path, source / include_path):
                    if candidate.is_file():
                        candidate = candidate.resolve()
                        if not candidate.is_relative_to(source):
                            raise ValueError(f"Include escapes upstream source: {include_path}")
                        pending.append(candidate)
                        break
        manifest = {"revision": revision, "boring_revision": boring_revision}
        if boring_version is not None:
            manifest["boring_version"] = boring_version
        manifest.update(sources=sources, files=dict(sorted(files.items())))
        (staged / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n")
        (staged / "README.md").write_text(
            "# BoringSSL PKI\n\nThe unmodified PKI target from BoringSSL commit\n"
            f"`{revision}`, the submodule used by our\n"
            f"`cloudflare/boring` revision `{boring_revision}`.\n"
            "Chromium uses this PKI implementation for certificate path building and RFC 5280\n"
            "verification. The Rust binding currently builds SSL and crypto but does not\n"
            "expose this separate target. We compile PKI only and link it to that same crypto\n"
            "instance; there is no second crypto implementation or legacy verifier fallback.\n\n"
            "Run `python scripts/vendor-boringssl-pki.py --check` to verify the byte hashes.\n"
            "To regenerate, pass `--source` pointing to the matching BoringSSL checkout.\n"
            "The checkout must be clean. Generation validates a fresh sibling staging tree\n"
            "before replacing the snapshot, removes obsolete upstream files and restores the\n"
            "previous directory if publication fails. Checks reject unlisted files and links.\n"
            "`manifest.json` records the upstream source list and exact file hashes. Update\n"
            "this snapshot together with the pinned boring family, never independently.\n\n"
            f"Upstream: https://boringssl.googlesource.com/boringssl/+/{revision}/pki/\n"
            "Licenses are in `LICENSE` and each original source header.\n",
            encoding="utf-8", newline="\n")
        check_snapshot(staged, revision=revision, boring_revision=boring_revision)
        replace_directory(staged, destination)
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--source", type=Path)
    selection.add_argument("--check", action="store_true")
    args = parser.parse_args()
    receipt = json.loads((DEST / "manifest.json").read_text(encoding="utf-8"))
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    for name in FAMILY:
        if cargo["patch"]["crates-io"][name]["rev"] != receipt["boring_revision"]:
            raise ValueError(f"PKI pin does not match {name}")
    if args.check:
        manifest = check_snapshot(DEST)
    else:
        dependency = cargo["workspace"]["dependencies"]["boring"]
        manifest = generate_snapshot(args.source, DEST, receipt["revision"], receipt["boring_revision"], dependency["version"].removeprefix("="))
    print(f"Verified {len(manifest['files'])} pinned BoringSSL PKI files")


if __name__ == "__main__":
    main()
