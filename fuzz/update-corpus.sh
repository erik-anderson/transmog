#!/usr/bin/env bash
set -euo pipefail

runs="${FUZZ_RUNS:-100000}"
max_total_time="${FUZZ_MAX_TOTAL_TIME:-}"
if [[ -z "$max_total_time" && ! "$runs" =~ ^[1-9][0-9]*$ ]]; then
    echo 'FUZZ_RUNS must be a positive integer.' >&2
    exit 2
fi
if [[ -n "$max_total_time" && ! "$max_total_time" =~ ^[1-9][0-9]*$ ]]; then
    echo 'FUZZ_MAX_TOTAL_TIME must be a positive integer when set.' >&2
    exit 2
fi

work_root="$(mktemp -d fuzz/.corpus-work.XXXXXX)"
trap 'rm -rf "$work_root"' EXIT

campaign_limit() {
    if [[ -n "$max_total_time" ]]; then
        printf '%s\n' "-max_total_time=$max_total_time"
    else
        printf '%s\n' "-runs=$runs"
    fi
}

evolve_target() {
    local target="$1"
    local max_len="$2"
    shift 2
    local work="$work_root/$target"
    local destination="fuzz/corpus/$target"
    mkdir -p "$work" "$destination"
    if [[ -d "$destination" ]]; then
        cp -a "$destination/." "$work/"
    fi
    cp -a "fuzz/seeds/$target/." "$work/"

    cargo +nightly-2026-10-01 fuzz run "$target" "$work" -- \
        -len_control=0 \
        -max_len="$max_len" \
        -rss_limit_mb=2048 \
        -timeout=10 \
        -verbosity=0 \
        "$(campaign_limit)" \
        "$@"
    cargo +nightly-2026-10-01 fuzz cmin "$target" "$work" -- \
        -max_len="$max_len" \
        -verbosity=0 \
        "$@"
}

publish_target() {
    local target="$1"
    local work="$work_root/$target"
    local destination="fuzz/corpus/$target"
    mkdir -p "$destination"
    find "$destination" -mindepth 1 -maxdepth 1 -type f -delete
    cp -a "$work/." "$destination/"
    printf 'Updated %s with %s minimized inputs.\n' \
        "$destination" "$(find "$destination" -maxdepth 1 -type f | wc -l)"
}

targets="${FUZZ_TARGETS:-content_encoding decode_stream codec_roundtrip content_pipeline}"
selected_targets=()
for target in $targets; do
    case "$target" in
        content_encoding)
            evolve_target content_encoding 16384 -dict=fuzz/dictionaries/content_encoding.dict
            ;;
        decode_stream)
            evolve_target decode_stream 8192
            ;;
        codec_roundtrip)
            evolve_target codec_roundtrip 16384
            ;;
        content_pipeline)
            evolve_target content_pipeline 8192
            ;;
        *)
            echo "Unknown fuzz target: $target" >&2
            exit 2
            ;;
    esac
    selected_targets+=("$target")
done

# Do not replace any checked-in corpus until every selected campaign and
# minimization succeeds. In particular, a crash leaves all prior corpora intact.
for target in "${selected_targets[@]}"; do
    publish_target "$target"
done
