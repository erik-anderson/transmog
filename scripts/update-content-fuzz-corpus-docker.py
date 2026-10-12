"""Update minimized fuzz corpora with portable Docker orchestration."""
import argparse
from docker_tools import build_image, container_arguments
from repo_tools import command, main_guard


def main():
    targets = ('content_encoding', 'decode_stream', 'codec_roundtrip', 'content_pipeline', 'websocket_frames', 'sse_stream', 'capture_reader', 'capture_crypto')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=100000)
    parser.add_argument("--target", nargs="+", choices=targets, default=list(targets))
    parser.add_argument("--image", default="transmog-content-fuzz:nightly-2026-10-01")
    args = parser.parse_args()
    if not 1 <= args.runs <= 1000000000:
        parser.error("--runs must be between 1 and 1000000000")
    docker = build_image(args.image)
    argv = container_arguments(docker, args.image, writable=True,
        environment=(f"FUZZ_RUNS={args.runs}", "FUZZ_TARGETS=" + " ".join(args.target)),
        mounts=("type=volume,source=transmog-content-fuzz-cargo-registry,target=/usr/local/cargo/registry",
                "type=volume,source=transmog-content-fuzz-target,target=/work/fuzz/target"))
    command([*argv, "bash", "fuzz/update-corpus.sh"])


if __name__ == "__main__":
    main_guard(main)
