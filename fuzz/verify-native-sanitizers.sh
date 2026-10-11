#!/usr/bin/env bash
set -euo pipefail

# Inspect the active ASan release build, not an unrelated coverage/build cache.
build_root="${FUZZ_NATIVE_BUILD_ROOT:-fuzz/target/x86_64-unknown-linux-gnu/release/build}"
verify() {
    local library="$1" pattern="$2" found=0
    while IFS= read -r archive; do
        local symbols
        symbols="$(nm -A "$archive" 2>/dev/null | grep -v 'rcgu\.o:' || true)"
        if grep -qE '__asan_(load|store|report)' <<<"$symbols" \
            && grep -qE '__sanitizer_cov_(8bit_counters|pcs|trace_cmp)' <<<"$symbols"; then
            echo "Verified native $library sanitizer instrumentation in $archive"
            found=1
        fi
    done < <(find "$build_root" -name "$pattern" -type f)
    if [[ "$found" != 1 ]]; then
        echo "No instrumented native $library archive in $build_root; rebuild with Clang sanitizer flags." >&2
        exit 1
    fi
}
verify zstd 'libzstd_sys-*.rlib'
if [[ "${1:-}" == capture ]]; then
    verify BoringSSL libcrypto.a
fi
