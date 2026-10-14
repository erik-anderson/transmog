"""Copy the PKI target and header closure from our exact boring-sys submodule.

No SSL or crypto implementation is built here. PKI links to boring-sys's crypto.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / "third_party" / "boringssl-pki"
REVISION = "427ec40cc8edc545253289232e885708c409e5ea"
BORING_REVISION = "9c57211afd4e99479e4fbba73cd603128726275b"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        manifest = json.loads((DEST / "manifest.json").read_text())
        if manifest["revision"] != REVISION or manifest["boring_revision"] != BORING_REVISION:
            raise SystemExit("PKI manifest does not match the pinned revisions")
        cargo = tomllib.loads((ROOT / "Cargo.toml").read_text())
        for name in ("boring", "boring-sys", "hyper-boring", "tokio-boring"):
            if cargo["patch"]["crates-io"][name]["rev"] != BORING_REVISION:
                raise SystemExit(f"PKI pin does not match {name}")
        for name in manifest["sources"]:
            if name not in manifest["files"] or not name.startswith("pki/") or not name.endswith(".cc"):
                raise SystemExit(f"Unexpected PKI source: {name}")
        for name, digest in manifest["files"].items():
            if hashlib.sha256((DEST / name).read_bytes()).hexdigest() != digest:
                raise SystemExit(f"PKI source hash mismatch: {name}")
        print(f"Verified {len(manifest['files'])} pinned BoringSSL PKI files")
        return
    if args.source is None:
        parser.error("--source is required when generating")
    source = args.source.resolve()
    actual = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    if actual != REVISION:
        raise SystemExit(f"Expected BoringSSL {REVISION}, got {actual}")
    cmake = (source / "gen/sources.cmake").read_text()
    block = re.search(r"set\(\s*PKI_SOURCES\s+(.*?)\n\)", cmake, re.S).group(1)
    sources = block.split()
    # Public crypto declarations must exactly match the linked boring-sys instance.
    pending = [source / name for name in sources]
    pending.extend((source / "include").rglob("*.h"))
    pending.extend((source / "pki").glob("*.h"))
    pending.append(source / "LICENSE")
    files: dict[str, str] = {}
    while pending:
        path = pending.pop()
        name = path.relative_to(source).as_posix()
        if name in files:
            continue
        data = path.read_bytes()
        target = DEST / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        files[name] = hashlib.sha256(data).hexdigest()
        for include in re.findall(rb'^\s*#\s*include\s*[<"]([^>"]+)[>"]', data, re.M):
            include_path = include.decode()
            for candidate in (path.parent / include_path, source / "include" / include_path, source / include_path):
                if candidate.is_file():
                    pending.append(candidate.resolve())
                    break
    manifest = {"revision": REVISION, "boring_revision": BORING_REVISION,
                "sources": sources, "files": dict(sorted(files.items()))}
    (DEST / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(f"Vendored {len(sources)} PKI sources and {len(files)} total files")


if __name__ == "__main__":
    main()
