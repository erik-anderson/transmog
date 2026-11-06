"""Prepare the pinned Sparkplug-only native V8 engine for Windows builds."""
import argparse
import platform

from repo_tools import command, development_environment, main_guard
from v8_native import locked_prepare as prepare

TARGETS = ("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepare-only", action="store_true", help="verify and stage source without compiling")
    parser.add_argument("--target", choices=TARGETS, default="aarch64-pc-windows-msvc" if platform.machine().lower() in {"arm64", "aarch64"} else "x86_64-pc-windows-msvc")
    args = parser.parse_args()
    if args.prepare_only:
        print(f"Verified native V8 source: {prepare()}")
    else:
        # Selecting the owning workspace crate preserves the product's V8
        # features instead of enabling the binding crate's upstream defaults.
        command(["cargo", "build", "--release", "--locked", "-p", "transmog-js-runtime", "--target", args.target],
                env=development_environment())
        print(f"Prepared Sparkplug-only V8 with Intl/Unicode support, without Temporal or Wasm: {args.target}")


if __name__ == "__main__":
    main_guard(main)
