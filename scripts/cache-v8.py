"""Prepare the pinned Sparkplug-only native V8 engine for Windows builds."""
import argparse
import platform
from pathlib import Path
import time

from repo_tools import command, development_environment, main_guard
from v8_native import locked_prepare as prepare

TARGETS = ("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verbose", action="store_true", help="stream native build-script progress")
    parser.add_argument("--export", type=Path, help="export a verified reusable engine for release jobs")
    parser.add_argument("--prepare-only", action="store_true", help="verify and stage source without compiling")
    parser.add_argument("--target", choices=TARGETS, default="aarch64-pc-windows-msvc" if platform.machine().lower() in {"arm64", "aarch64"} else "x86_64-pc-windows-msvc")
    args = parser.parse_args()
    if args.prepare_only:
        print(f"Verified native V8 source: {prepare()}")
    else:
        # Selecting the owning workspace crate preserves the product's V8
        # features instead of enabling the binding crate's upstream defaults.
        started = time.monotonic()
        environment = development_environment(target=args.target)
        if args.export:
            environment["TRANSMOG_V8_EXPORT"] = str(args.export.resolve())
        command(["cargo", "build", "--release", "--locked", "-p", "transmog-js-runtime", "--target", args.target, *(["-vv"] if args.verbose else [])],
                env=environment)
        if args.export and not (args.export / "manifest.json").is_file():
            raise RuntimeError("Cargo did not export the prepared V8 artifact")
        print(f"V8 preparation duration: {time.monotonic() - started:.1f}s", flush=True)
        print(f"Prepared Sparkplug-only V8 with Intl/Unicode support, without Temporal or Wasm: {args.target}")


if __name__ == "__main__":
    main_guard(main)
