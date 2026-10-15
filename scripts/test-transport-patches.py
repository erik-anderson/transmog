"""Run focused upstream-crate regressions in an isolated copy with our TLS pins."""
import argparse
import json
from pathlib import Path
import shutil
import tempfile
import tomllib

from repo_tools import ROOT, command, development_environment, main_guard
from tls_dependencies import FAMILY, verify_stack
from transport_dependencies import check_repository


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", choices=("h2", "hyper", "quiche"), default="quiche")
    parser.add_argument("--filter", default="transmog_")
    parser.add_argument("--update-lock", action="store_true", help="Review and save test-only dependency resolution after an upstream roll")
    args = parser.parse_args()
    check_repository()
    parent = ROOT / ".local/transport-patch-tests"
    parent.mkdir(parents=True, exist_ok=True)
    environment = development_environment()
    environment["CARGO_TARGET_DIR"] = str(ROOT / "target/transport-patch-tests")
    environment["TRANSMOG_BUILD_CACHE_TRIM"] = "0"
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    with tempfile.TemporaryDirectory(prefix="case-", dir=parent) as temporary:
        directory = Path(temporary)
        for name in ("h2", "hyper", "quiche"):
            shutil.copytree(ROOT / "third_party/rust-transport/vendor" / name, directory / name)
        manifest = '[workspace]\nresolver = "2"\nmembers = ["h2", "hyper", "quiche"]\n[patch.crates-io]\n'
        for name in FAMILY:
            pin = cargo["patch"]["crates-io"][name]
            manifest += f'{name} = {{ git = {json.dumps(pin["git"])}, rev = {json.dumps(pin["rev"])} }}\n'
        for name in ("h2", "hyper", "quiche"):
            manifest += f'{name} = {{ path = "{name}" }}\n'
        (directory / "Cargo.toml").write_text(manifest, encoding="utf-8")
        lock = ROOT / "third_party/rust-transport/test-Cargo.lock"
        if lock.is_file():
            shutil.copyfile(lock, directory / "Cargo.lock")
        elif not args.update_lock:
            raise ValueError("Missing test dependency lock; use --update-lock and review it")
        locked = [] if args.update_lock else ["--locked"]
        metadata = json.loads(command(["cargo", "metadata", "--format-version", "1", *locked], cwd=directory, env=environment, capture=True).stdout)
        verify_stack(metadata, ROOT)
        command(["cargo", "test", "--locked", "-p", args.package, "--lib", args.filter], cwd=directory / args.package, env=environment)
        if args.update_lock:
            shutil.copyfile(directory / "Cargo.lock", lock)


if __name__ == "__main__":
    main_guard(main)
