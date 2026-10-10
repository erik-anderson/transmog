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

URL = "https://github.com/denoland/rusty_v8/releases/download/v150.4.0/rusty_v8_simdutf_release_x86_64-pc-windows-msvc.lib.gz"
SHA256 = "f231f82cbacb9aefe6d9af57e6df2e8959a40e001f79306485133e3c075b98f0"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive-path", type=Path)
    args = parser.parse_args()
    cache = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))) / ".rusty_v8"
    target = cache / re.sub(r"[^A-Za-z0-9]", "_", URL)
    cache.mkdir(parents=True, exist_ok=True)
    source = args.archive_path or (target if target.is_file() else None)
    # Stage within the cache, verify before atomically replacing its final entry.
    with tempfile.TemporaryDirectory(prefix="v8-", dir=cache) as temporary:
        staged = Path(temporary) / "archive.gz"
        if source:
            shutil.copyfile(source, staged)
        else:
            with urlopen(URL, timeout=60) as response, staged.open("wb") as output:
                shutil.copyfileobj(response, output)
        with staged.open("rb") as archive:
            digest = hashlib.file_digest(archive, "sha256").hexdigest()
        if digest != SHA256:
            raise ValueError(f"rusty_v8 archive SHA-256 mismatch: {digest}")
        if source is None or source.resolve() != target.resolve():
            staged.replace(target)
    print(f"rusty_v8 150.4.0: SHA-256 verified; cache path {target}")


if __name__ == "__main__":
    main_guard(main)
