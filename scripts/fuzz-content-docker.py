"""Run bounded content fuzz campaigns in an owned Docker container."""
import argparse
from docker_tools import build_image, container_arguments
from repo_tools import ROOT, command, main_guard


def main():
    targets = ('content_encoding', 'decode_stream', 'codec_roundtrip', 'content_pipeline', 'websocket_frames', 'sse_stream', 'capture_reader', 'capture_crypto', 'script_compiler', 'script_protocol')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=10000)
    parser.add_argument("--image", default="transmog-content-fuzz:nightly-2026-10-01")
    parser.add_argument("--target", nargs="+", choices=targets, default=list(targets))
    args = parser.parse_args()
    if not 1 <= args.runs <= 1000000000:
        parser.error("--runs must be between 1 and 1000000000")
    artifacts = (ROOT / "fuzz" / "artifacts").resolve()
    if not artifacts.is_relative_to(ROOT):
        raise RuntimeError("Fuzz artifacts must stay inside the repository.")
    artifacts.mkdir(exist_ok=True)
    docker = build_image(args.image)
    argv = container_arguments(docker, args.image, environment=(f"FUZZ_RUNS={args.runs}", "FUZZ_TARGETS=" + " ".join(args.target)),
        mounts=("type=volume,source=transmog-content-fuzz-cargo-registry,target=/usr/local/cargo/registry",
                "type=volume,source=transmog-content-fuzz-target,target=/work/fuzz/target",
                f"type=bind,source={artifacts},target=/work/fuzz/artifacts"))
    command([*argv, "bash", "fuzz/run-container.sh"])


if __name__ == "__main__":
    main_guard(main)
