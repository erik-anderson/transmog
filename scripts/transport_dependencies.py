"""Check or reproduce narrowly patched registry transport crates.

Edit sources.json and review the patch when rolling a crate. --regenerate
downloads only the exact checksum-pinned archives, or uses --archive-dir.
"""
from __future__ import annotations

import argparse
import difflib
import hashlib
import io
import json
import re
from pathlib import Path
import subprocess
import tarfile
import tempfile
import urllib.request

from dependency_snapshots import replace_directory, snapshot_file
from repo_tools import ROOT, main_guard

BASE = ROOT / "third_party/rust-transport"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def inventory(directory):
    result = {}
    for path in directory.rglob("*"):
        name = path.relative_to(directory).as_posix()
        snapshot_file(directory, name)
        if path.is_file():
            result[name] = digest(path.read_bytes())
    return dict(sorted(result.items()))


def unpack(data, destination, prefix):
    total = 0
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        seen = set()
        for member in archive:
            if not member.name.startswith(prefix + "/"):
                raise ValueError("Archive has an unexpected root")
            name = member.name[len(prefix) + 1:]
            path = snapshot_file(destination, name)
            if member.isdir():
                continue
            if not member.isfile() or name in seen or member.size > 8 * 1024 * 1024:
                raise ValueError("Archive contains a link, duplicate or oversized file")
            total += member.size
            if total > 32 * 1024 * 1024:
                raise ValueError("Archive exceeds its unpacked size bound")
            seen.add(name)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(archive.extractfile(member).read())


def regenerate(sources, archive_dir):
    vendor = BASE / "vendor"
    vendor.mkdir(exist_ok=True)
    for name, source in sources.items():
        filename = f'{name}-{source["version"]}.crate'
        if archive_dir:
            data = (archive_dir / filename).read_bytes()
        else:
            with urllib.request.urlopen(f"https://static.crates.io/crates/{name}/{filename}", timeout=30) as response:
                data = response.read(8 * 1024 * 1024 + 1)
        if len(data) > 8 * 1024 * 1024 or digest(data) != source["archiveSha256"]:
            raise ValueError(f"Upstream archive checksum differs: {name}")
        with tempfile.TemporaryDirectory(prefix=".transport-stage-", dir=vendor) as temporary:
            staged = Path(temporary)
            unpack(data, staged, f'{name}-{source["version"]}')
            patch = BASE / "patches" / f"{name}.patch"
            patch_data = patch.read_bytes()
            if patch_data:
                # Prevent Git from treating the parent repository as this
                # crate's working tree. No hooks or commits are involved.
                subprocess.run(["git", "init", "--quiet", str(staged)], check=True)
                apply = ["git", "-c", "core.autocrlf=false", "-c", "core.eol=lf", "-C", str(staged), "apply"]
                subprocess.run([*apply, "--check", str(patch)], check=True)
                subprocess.run([*apply, str(patch)], check=True)
                # Only this freshly created, verified staging metadata is removed.
                import shutil
                shutil.rmtree(staged / ".git")
            receipt = {"source": source, "patchSha256": digest(patch_data), "files": inventory(staged)}
            (staged / "transmog-snapshot.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8", newline="\n")
            replace_directory(staged, vendor / name)


def record_patches(sources, archive_dir):
    if archive_dir is None:
        raise ValueError("--record-patches requires the original --archive-dir")
    for name, source in sources.items():
        data = (archive_dir / f'{name}-{source["version"]}.crate').read_bytes()
        if digest(data) != source["archiveSha256"]:
            raise ValueError(f"Upstream archive checksum differs: {name}")
        with tempfile.TemporaryDirectory(prefix=".transport-original-", dir=BASE) as temporary:
            original = Path(temporary)
            unpack(data, original, f'{name}-{source["version"]}')
            current = BASE / "vendor" / name
            files = set(inventory(original)) | set(inventory(current))
            files.discard("transmog-snapshot.json")
            changes = []
            for filename in sorted(files):
                old, new = original / filename, current / filename
                before = old.read_bytes() if old.exists() else b""
                after = new.read_bytes() if new.exists() else b""
                if before != after:
                    changes.extend(difflib.unified_diff(
                        before.decode("utf-8").replace("\r\n", "\n").splitlines(keepends=True),
                        after.decode("utf-8").replace("\r\n", "\n").splitlines(keepends=True),
                        fromfile="a/" + filename if old.exists() else "/dev/null",
                        tofile="b/" + filename if new.exists() else "/dev/null"))
            (BASE / "patches" / f"{name}.patch").write_text("".join(changes), encoding="utf-8", newline="\n")


def check(sources):
    for name, source in sources.items():
        directory = BASE / "vendor" / name
        receipt = json.loads((directory / "transmog-snapshot.json").read_text(encoding="utf-8"))
        if receipt["source"] != source or receipt["patchSha256"] != digest((BASE / "patches" / f"{name}.patch").read_bytes()):
            raise ValueError(f"Transport source or patch receipt differs: {name}")
        actual = inventory(directory)
        actual.pop("transmog-snapshot.json")
        if actual != receipt["files"]:
            raise ValueError(f"Transport snapshot file inventory differs: {name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--regenerate", action="store_true")
    parser.add_argument("--archive-dir", type=Path)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--record-patches", action="store_true", help="Record deliberate vendor edits, then reproduce them from the original archives")
    args = parser.parse_args()
    sources = json.loads((BASE / "sources.json").read_text(encoding="utf-8"))
    validate_sources(sources)
    if args.record_patches:
        record_patches(sources, args.archive_dir)
    if args.regenerate or args.record_patches:
        regenerate(sources, args.archive_dir)
    check(sources)
    print("Transport dependency snapshots verified")


def validate_sources(sources):
    if set(sources) != {"h2", "hyper", "quiche"}:
        raise ValueError("Unexpected transport crate set")
    for source in sources.values():
        if set(source) != {"version", "archiveSha256"} or not re.fullmatch(r"\d+\.\d+\.\d+", source["version"]) or not re.fullmatch(r"[0-9a-f]{64}", source["archiveSha256"]):
            raise ValueError("Transport sources require exact versions and SHA-256 checksums")


def check_repository(root=ROOT, metadata=None):
    global BASE
    previous = BASE
    try:
        BASE = root / "third_party/rust-transport"
        sources = json.loads((BASE / "sources.json").read_text(encoding="utf-8"))
        validate_sources(sources)
        check(sources)
        if metadata is not None:
            for name, source in sources.items():
                packages = [p for p in metadata["packages"] if p["name"] == name]
                for package in packages:
                    expected = (BASE / "vendor" / name / "Cargo.toml").resolve()
                    if package["version"] != source["version"] or package.get("source") is not None or Path(package["manifest_path"]).resolve() != expected:
                        raise ValueError(f"Resolved {name} does not use the reviewed transport snapshot")
    finally:
        BASE = previous


if __name__ == "__main__":
    main_guard(main)
