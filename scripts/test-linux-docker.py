"""Validate the complete Rust workspace as native Linux binaries in Docker."""
import argparse
from docker_tools import build_image, container_arguments
from repo_tools import command, main_guard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", default="transmog-linux-tools:rust-1.97.1-nightly-2026-10-01")
    args = parser.parse_args()
    docker = build_image(args.image)
    argv = container_arguments(docker, args.image,
        environment=("CFLAGS=", "CXXFLAGS=", "CARGO_TARGET_DIR=/work-target"),
        mounts=("type=volume,source=transmog-linux-cargo-registry,target=/usr/local/cargo/registry",
                "type=volume,source=transmog-linux-target,target=/work-target"))
    command([*argv, "bash", "-c", "set -euo pipefail; cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features --locked -- -D warnings && cargo test --workspace --all-features --all-targets --locked && cargo build --release --workspace --all-features --locked"])


if __name__ == "__main__":
    main_guard(main)
