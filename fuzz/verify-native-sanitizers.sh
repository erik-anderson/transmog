#!/usr/bin/env bash
set -euo pipefail

mapfile -t archives < <(
    find fuzz/target -path '*/release/build/zstd-sys/*/out/libzstd_sys-*.rlib' -type f
)
if [[ "${#archives[@]}" -eq 0 ]]; then
    echo 'No zstd-sys archive was found; build a fuzz target first.' >&2
    exit 2
fi

for archive in "${archives[@]}"; do
    symbols="$(nm -A "$archive" 2>/dev/null | grep -v 'rcgu\.o:' || true)"
    if grep -qE '__asan_(load|store|report)' <<<"$symbols" \
        && grep -qE '__sanitizer_cov_(8bit_counters|pcs|trace_cmp)' <<<"$symbols"; then
        echo "Verified native sanitizer instrumentation in $archive"
        exit 0
    fi
done

echo 'zstd native objects do not contain both ASan and sanitizer-coverage instrumentation.' >&2
exit 1
