"""Qualify the native SDK in the repository's Linux image (including WSL2 Docker)."""
import argparse

from docker_tools import build_image, container_arguments
from repo_tools import ROOT, command, main_guard, write_json


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", default="transmog-linux-tools:rust-1.99.1-nightly-2026-10-01")
    args = parser.parse_args()
    docker = build_image(args.image)
    reports = ROOT / ".local/sdk-linux-qualification"
    reports.mkdir(parents=True, exist_ok=True)
    image = command([docker, "image", "inspect", args.image, "--format", "{{.Id}}"], capture=True).stdout.strip()
    kernel = command([docker, "info", "--format", "{{.KernelVersion}}"], capture=True).stdout.strip()
    write_json(reports / "host.json", {"image": args.image, "image_id": image, "kernel": kernel})
    mounts = (
        "type=volume,source=transmog-sdk-linux-tools,target=/work/.tools",
        "type=volume,source=transmog-sdk-linux-local,target=/work/.local",
        "type=volume,source=transmog-linux-cargo-registry,target=/usr/local/cargo/registry",
        "type=volume,source=transmog-linux-cargo-git,target=/usr/local/cargo/git",
        "type=volume,source=transmog-sdk-linux-managed-bin,target=/work/examples/dotnet-embedding/Transmog.Embedding.Sample/bin",
        "type=volume,source=transmog-sdk-linux-managed-obj,target=/work/examples/dotnet-embedding/Transmog.Embedding.Sample/obj",
        f"type=bind,source={reports},target=/qualification",
    )
    argv = container_arguments(docker, image, mounts=mounts,
        environment=("CFLAGS=", "CXXFLAGS=", "RUSTUP_TOOLCHAIN=1.99.1", "PYTHONUNBUFFERED=1",
                     "CARGO_TARGET_DIR=/work/.local/sdk/rust-target", "CARGO_BUILD_JOBS=4",
                     "CMAKE_BUILD_PARALLEL_LEVEL=4", "DOTNET_CLI_TELEMETRY_OPTOUT=1",
                     "DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1", "GIT_OPTIONAL_LOCKS=0"))
    command([*argv, "python3", "/work/e2e/sdk-linux/qualify.py"])
    print(f"SDK_LINUX_QUALIFICATION={reports}")


if __name__ == "__main__":
    main_guard(main)
