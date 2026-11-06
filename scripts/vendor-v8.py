"""Verify or record the small V8 binding fork against its pinned source archive."""
import argparse
import difflib
import hashlib
import json
import tarfile

from repo_tools import main_guard
from v8_dependencies import NATIVE_REVISION, check_repository
from v8_native import VENDOR, source_archive, source_pin


def write_changed(path, data):
    if not path.is_file() or path.read_bytes() != data:
        path.write_bytes(data)


def record():
    pin = source_pin()
    patch = []
    with tarfile.open(source_archive(pin)) as archive:
        originals = {m.name.removeprefix(f'v8-{pin["version"]}/'): m for m in archive if m.isfile()}
        inventory = {name: hashlib.sha256(archive.extractfile(member).read()).hexdigest()
                     for name, member in originals.items()}
        source_receipt = json.dumps({"archive_sha256": pin["archive_sha256"], "files": inventory}, indent=2) + "\n"
        write_changed(VENDOR / "source-receipt.sha256", (hashlib.sha256(source_receipt.encode("utf-8")).hexdigest() + "\n").encode("ascii"))
        for file in sorted(VENDOR.rglob("*")):
            name = file.relative_to(VENDOR).as_posix()
            if not file.is_file() or name in {"bindings.patch", "transmog-snapshot.json"}:
                continue
            original = archive.extractfile(originals[name]).read().decode("utf-8") if name in originals else ""
            current = file.read_bytes().decode("utf-8")
            if current != original:
                patch.extend(difflib.unified_diff(original.splitlines(True), current.splitlines(True),
                             fromfile="a/" + name if original else "/dev/null", tofile="b/" + name))
    write_changed(VENDOR / "bindings.patch", "".join(patch).encode("utf-8"))
    files = {p.relative_to(VENDOR).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
             for p in sorted(VENDOR.rglob("*")) if p.is_file() and p.name != "transmog-snapshot.json"}
    receipt = {"source": pin, "nativeRevision": NATIVE_REVISION, "files": files}
    write_changed(VENDOR / "transmog-snapshot.json", (json.dumps(receipt, indent=2) + "\n").encode("utf-8"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", action="store_true", help="record an intentionally reviewed local binding/profile change")
    args = parser.parse_args()
    if args.record:
        record()
    check_repository()
    print("V8 binding and Sparkplug native profile snapshot verified")


if __name__ == "__main__":
    main_guard(main)
