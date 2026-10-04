#!/usr/bin/env bash
set -euo pipefail

runs="${FUZZ_RUNS:-10000}"
if [[ ! "$runs" =~ ^[1-9][0-9]*$ ]]; then
    echo 'FUZZ_RUNS must be a positive integer.' >&2
    exit 2
fi

corpus_root="$(mktemp -d)"
trap 'rm -rf "$corpus_root"' EXIT
mkdir -p "$corpus_root/content_encoding" "$corpus_root/decode_stream"
cp -a fuzz/corpus/content_encoding/. "$corpus_root/content_encoding/"
cp -a fuzz/corpus/decode_stream/. "$corpus_root/decode_stream/"

cargo +nightly-2026-10-01 fuzz run \
    content_encoding "$corpus_root/content_encoding" -- \
    -dict=fuzz/dictionaries/content_encoding.dict \
    -max_len=16384 \
    -runs="$runs"
cargo +nightly-2026-10-01 fuzz run \
    decode_stream "$corpus_root/decode_stream" -- \
    -max_len=8192 \
    -runs="$runs"
