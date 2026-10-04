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
        -runs="$runs" \
        "$@"
}

run_target content_encoding 16384 -dict=fuzz/dictionaries/content_encoding.dict
run_target decode_stream 8192
run_target codec_roundtrip 16384
run_target content_pipeline 8192
