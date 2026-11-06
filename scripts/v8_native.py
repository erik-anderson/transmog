"""Prepare checksum-pinned native sources for Transmog's V8 profile."""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import tarfile
import tempfile
from urllib.request import urlopen

from dependency_snapshots import snapshot_file
from repo_tools import ROOT, main_guard

VENDOR = ROOT / "third_party/rusty-v8"


def digest(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def source_pin():
    return json.loads((VENDOR / "source.json").read_text(encoding="utf-8"))


def source_archive(pin, *, cargo_home=None):
    filename = f'v8-{pin["version"]}.crate'
    home = Path(cargo_home or os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    candidates = [*home.glob(f"registry/cache/*/{filename}"), ROOT / ".tools/downloads" / filename]
    for archive in candidates:
        if archive.is_file():
            if digest(archive) != pin["archive_sha256"]:
                raise ValueError("V8 registry archive checksum differs from the source pin")
            return archive
    archive = candidates[-1]
    archive.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="v8-download-", dir=archive.parent) as temporary:
        staged = Path(temporary) / filename
        with urlopen(pin["archive_url"], timeout=60) as response, staged.open("wb") as output:
            shutil.copyfileobj(response, output)
        if digest(staged) != pin["archive_sha256"]:
            raise ValueError("Downloaded V8 registry archive checksum differs from the source pin")
        staged.replace(archive)
    return archive


def unpack(archive, destination, prefix):
    """Extract regular source files with bounded sizes and no links/traversal."""
    inventory = {}
    total = 0
    with tarfile.open(archive, "r:gz") as source:
        for member in source:
            name = PurePosixPath(member.name)
            if not name.parts or name.parts[0] != prefix:
                raise ValueError("V8 archive has an unexpected root")
            parts = name.parts[1:]
            if not parts or member.isdir():
                continue
            if (not member.isfile() or any(part in {".", ".."} or ":" in part or "\\" in part for part in parts)
                    or member.size > 32 * 1024 * 1024):
                raise ValueError("V8 archive contains an unsafe entry")
            relative = "/".join(parts)
            if relative in inventory:
                raise ValueError("V8 archive contains a duplicate file")
            total += member.size
            if total > 256 * 1024 * 1024:
                raise ValueError("V8 archive exceeds its source size bound")
            target = destination.joinpath(*parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            data = source.extractfile(member).read()
            target.write_bytes(data)
            inventory[relative] = hashlib.sha256(data).hexdigest()
    return inventory


def prepare():
    from v8_dependencies import check_repository
    check_repository()
    pin = source_pin()
    archive = source_archive(pin)
    overlays = patched_files(archive, pin)
    inputs = {
        "format": 1,
        "archiveSha256": pin["archive_sha256"],
        "nativePatchSha256": digest(VENDOR / "native.patch"),
        "icuDataSha256": pin["icuData"]["filteredSha256"],
    }
    from v8_cache import source_name
    parent = ROOT / ".tools/rusty-v8/sources"
    parent.mkdir(parents=True, exist_ok=True)
    source = parent / source_name(pin["version"], inputs)
    receipt = source / ".transmog-source.json"
    if not source.exists():
        with tempfile.TemporaryDirectory(prefix=".v8-source-", dir=parent) as temporary:
            staged = Path(temporary) / "source"
            staged.mkdir()
            inventory = unpack(archive, staged, f'v8-{pin["version"]}')
            (staged / receipt.name).write_text(
                json.dumps({"archive_sha256": pin["archive_sha256"], "files": inventory}, indent=2) + "\n",
                encoding="utf-8", newline="\n",
            )
            for relative, data in overlays.items():
                snapshot_file(staged, relative).write_bytes(data)
            prepare_icu_data(staged, pin["icuData"])
            (staged / ".transmog-inputs.json").write_text(
                json.dumps(inputs, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
            verify_source(staged, pin, overlays, inputs)
            # Both paths are newly created descendants of the explicit tool root.
            staged.resolve().relative_to(parent.resolve())
            source.resolve().relative_to(parent.resolve())
            try:
                staged.rename(source)
            except OSError:
                # Another preparation may publish the same immutable tree first.
                # Validate that winner below; never replace an existing tree.
                if not source.is_dir() or source.is_symlink():
                    raise
    verify_source(source, pin, overlays, inputs)
    return source


def verify_source(source, pin, overlays, inputs):
    receipt = source / ".transmog-source.json"
    if json.loads((source / ".transmog-inputs.json").read_text(encoding="utf-8")) != inputs:
        raise ValueError("V8 native source cache has different inputs")
    if digest(receipt) != (VENDOR / "source-receipt.sha256").read_text().strip():
        raise ValueError("V8 source inventory differs from the reviewed registry archive")
    evidence = json.loads(receipt.read_text(encoding="utf-8"))
    if evidence["archive_sha256"] != pin["archive_sha256"]:
        raise ValueError("V8 source receipt differs from the pinned archive")
    for relative, expected in evidence["files"].items():
        if relative in overlays:
            expected = hashlib.sha256(overlays[relative]).hexdigest()
        path = snapshot_file(source, relative)
        if not path.is_file() or digest(path) != expected:
            raise ValueError(f"V8 source differs from its archive: {relative}")
    icu_data = snapshot_file(source, "third_party/icu/common/icudtl.dat")
    if not icu_data.is_file() or digest(icu_data) != pin["icuData"]["filteredSha256"]:
        raise ValueError("V8 source cache has different filtered ICU data")


def prepare_icu_data(source, pin):
    """Restore the archive's omitted ICU bundle with the reviewed data filter."""
    from icu_data import filter_package
    target = snapshot_file(source, "third_party/icu/common/icudtl.dat")
    if target.is_file() and digest(target) == pin["filteredSha256"]:
        return
    cache = ROOT / ".tools/downloads" / f'icu-{pin["revision"]}-icudtl.dat'
    cache.parent.mkdir(parents=True, exist_ok=True)
    if not cache.is_file():
        with tempfile.TemporaryDirectory(prefix="icu-download-", dir=cache.parent) as temporary:
            with urlopen(pin["url"], timeout=60) as response:
                encoded = response.read(64 * 1024 * 1024 + 1)
            if len(encoded) > 64 * 1024 * 1024:
                raise ValueError("ICU data download exceeds its size bound")
            data = base64.b64decode(encoded)
            if len(data) != pin["bytes"] or hashlib.sha256(data).hexdigest() != pin["sha256"]:
                raise ValueError("Downloaded ICU data differs from its reviewed pin")
            staged = Path(temporary) / cache.name
            staged.write_bytes(data)
            staged.replace(cache)
    if cache.stat().st_size != pin["bytes"] or digest(cache) != pin["sha256"]:
        raise ValueError("Cached ICU data differs from its reviewed pin")
    filtered = filter_package(cache.read_bytes(), pin["filter"])
    if len(filtered) != pin["filteredBytes"] or hashlib.sha256(filtered).hexdigest() != pin["filteredSha256"]:
        raise ValueError("Filtered ICU data differs from its reviewed pin")
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".icu-data-", dir=target.parent) as temporary:
        staged = Path(temporary) / target.name
        staged.write_bytes(filtered)
        staged.replace(target)


def patched_files(archive, pin):
    """Check/apply the complete native delta in a disposable Git root."""
    patch = VENDOR / "native.patch"
    paths = [line[6:] for line in patch.read_text().splitlines() if line.startswith("--- a/")]
    if len(paths) != len(set(paths)):
        raise ValueError("Invalid V8 native patch paths")
    parent = ROOT / ".tools/rusty-v8"
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".v8-patch-", dir=parent) as temporary:
        stage = Path(temporary)
        with tarfile.open(archive) as original:
            for name in paths:
                target = snapshot_file(stage, name)
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(original.extractfile(f'v8-{pin["version"]}/{name}').read())
        subprocess.run(["git", "init", "--quiet", str(stage)], check=True, capture_output=True)
        apply = ["git", "-c", "core.autocrlf=false", "-C", str(stage), "apply"]
        subprocess.run([*apply, "--check", str(patch)], check=True, capture_output=True)
        subprocess.run([*apply, str(patch)], check=True, capture_output=True)
        result = {name: (stage / name).read_bytes() for name in paths}
    if result.get("src/binding.cc") != (VENDOR / "src/binding.cc").read_bytes():
        raise ValueError("Vendored V8 adapter differs from the recorded native patch")
    return result


def locked_prepare():
    from v8_cache import cache_lease
    with cache_lease(ROOT):
        return prepare()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lock-held", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    print(prepare() if args.lock_held else locked_prepare())


if __name__ == "__main__":
    main_guard(main)
