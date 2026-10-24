"""Verify and cache the pinned Windows rusty_v8 archive from any host."""
import argparse
import hashlib
import os
from pathlib import Path
import re
import shutil
import tempfile
from urllib.request import urlopen
from repo_tools import main_guard
import platform

ARCHIVES = {
    "x86_64-pc-windows-msvc": "0b17ca072bae37dd4ff00e6014d2b413becb031c9342ee11cb8226a5881f62b2",
    "aarch64-pc-windows-msvc": "cad105f510f59dd38a9cbcc8f6f78263d9bb8f009d0c111f972fbd529de92273",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive-path", type=Path)
    parser.add_argument("--target", choices=ARCHIVES, default="aarch64-pc-windows-msvc" if platform.machine().lower() in {"arm64", "aarch64"} else "x86_64-pc-windows-msvc")
    args = parser.parse_args()
    url = f"https://github.com/denoland/rusty_v8/releases/download/v152.2.0/rusty_v8_release_{args.target}.lib.gz"
    cache = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))) / ".rusty_v8"
    target = cache / re.sub(r"[^A-Za-z0-9]", "_", url)
    cache.mkdir(parents=True, exist_ok=True)
    source = args.archive_path or (target if target.is_file() else None)
    # Stage within the cache, verify before atomically replacing its final entry.
    with tempfile.TemporaryDirectory(prefix="v8-", dir=cache) as temporary:
        staged = Path(temporary) / "archive.gz"
        if source:
            shutil.copyfile(source, staged)
        else:
            with urlopen(url, timeout=60) as response, staged.open("wb") as output:
                shutil.copyfileobj(response, output)
        with staged.open("rb") as archive:
            digest = hashlib.file_digest(archive, "sha256").hexdigest()
        if digest != ARCHIVES[args.target]:
            raise ValueError(f"rusty_v8 archive SHA-256 mismatch: {digest}")
        if source is None or source.resolve() != target.resolve():
            staged.replace(target)
    print(f"rusty_v8 152.2.0: SHA-256 verified; cache path {target}")


if __name__ == "__main__":
    main_guard(main)
