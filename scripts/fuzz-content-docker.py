"""Run bounded content fuzz campaigns in an owned Docker container."""
import argparse
from docker_tools import build_image, container_arguments
from repo_tools import command, main_guard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=10000)
    parser.add_argument("--image", default="transmog-content-fuzz:nightly-2026-10-01")
    args = parser.parse_args()
    if not 1 <= args.runs <= 1000000000:
        parser.error("--runs must be between 1 and 1000000000")
    docker = build_image(args.image)
    argv = container_arguments(docker, args.image, environment=(f"FUZZ_RUNS={args.runs}",),
        mounts=("type=volume,source=transmog-content-fuzz-cargo-registry,target=/usr/local/cargo/registry",
                "type=volume,source=transmog-content-fuzz-target,target=/work/fuzz/target",
                "type=tmpfs,target=/work/fuzz/artifacts"))
    command([*argv, "bash", "fuzz/run-container.sh"])


if __name__ == "__main__":
    main_guard(main)
