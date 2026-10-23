#!/usr/bin/env bash
set -euo pipefail

runs="${FUZZ_RUNS:-10000}"
if [[ ! "$runs" =~ ^[1-9][0-9]*$ ]]; then
    echo 'FUZZ_RUNS must be a positive integer.' >&2
    exit 2
fi

corpus_root="$(mktemp -d)"
trap 'rm -rf "$corpus_root"' EXIT

prepare_corpus() {
    local target="$1"
    mkdir -p "$corpus_root/$target"
    if [[ -d "fuzz/corpus/$target" ]]; then
        cp -a "fuzz/corpus/$target/." "$corpus_root/$target/"
    fi
    cp -a "fuzz/seeds/$target/." "$corpus_root/$target/"
}

run_target() {
    local target="$1"
    local max_len="$2"
    shift 2
    prepare_corpus "$target"
    cargo +nightly-2026-10-01 fuzz run "$target" "$corpus_root/$target" -- \
        -len_control=0 \
        -max_len="$max_len" \
        -rss_limit_mb=2048 \
        -timeout=10 \
        -verbosity=0 \
        -print_final_stats=1 \
        -runs="$runs" \
        "$@"
}

targets="${FUZZ_TARGETS:-content_encoding decode_stream codec_roundtrip content_pipeline websocket_frames sse_stream capture_reader capture_crypto script_compiler script_protocol}"
capture_selected=0
for target in $targets; do
    case "$target" in
        content_encoding) run_target "$target" 16384 -dict=fuzz/dictionaries/content_encoding.dict ;;
        decode_stream|content_pipeline|websocket_frames|sse_stream) run_target "$target" 8192 ;;
        script_compiler) run_target "$target" 16384 -dict=fuzz/dictionaries/script_compiler.dict ;;
        script_protocol) run_target "$target" 16384 -dict=fuzz/dictionaries/script_protocol.dict ;;
        codec_roundtrip) run_target "$target" 16384 ;;
        capture_reader) run_target "$target" 65537; capture_selected=1 ;;
        capture_crypto) run_target "$target" 256; capture_selected=1 ;;
        *) echo "Unknown fuzz target: $target" >&2; exit 2 ;;
    esac
done
if [[ "$capture_selected" == 1 ]]; then
    bash fuzz/verify-native-sanitizers.sh capture
fi
