#!/usr/bin/env bash
set -euo pipefail

target_dir="fuzz/target/coverage"
summary="${FUZZ_COVERAGE_SUMMARY:-fuzz/coverage-summary.txt}"
read -r -a targets <<<"${FUZZ_COVERAGE_TARGETS:-content_encoding decode_stream codec_roundtrip content_pipeline websocket_frames script_compiler script_protocol}"

for target in "${targets[@]}"; do
    corpora=("fuzz/seeds/$target")
    if [[ -d "fuzz/corpus/$target" ]]; then corpora+=("fuzz/corpus/$target"); fi
    replay_log="$(mktemp)"
    if ! cargo +nightly-2026-10-01 fuzz coverage \
        --target-dir "$target_dir" \
        "$target" \
        "${corpora[@]}" -- \
        -verbosity=0 >"$replay_log" 2>&1; then
        cat "$replay_log" >&2
        rm -f "$replay_log"
        exit 1
    fi
    tail -n 4 "$replay_log"
    rm -f "$replay_log"
done

llvm_cov="$(rustc +nightly-2026-10-01 --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-cov"
if [[ ! -x "$llvm_cov" ]]; then
    echo 'llvm-tools-preview is required for fuzz coverage reporting.' >&2
    exit 2
fi

{
    for target in "${targets[@]}"; do
        case "$target" in
            script_compiler)
                minimum_lines=""
                sources=("$PWD/crates/script-compiler/src/lib.rs" "$PWD/crates/proxy-script/src/lib.rs")
                ;;
            script_protocol)
                minimum_lines=""
                sources=("$PWD/crates/extensions/src/protocol.rs" "$PWD/crates/proxy-script/src/lib.rs")
                ;;
            content_encoding)
                minimum_lines=70
                sources=(
                    "$PWD/crates/proxy-content/src/coding.rs"
                    "$PWD/crates/proxy-core/src/header.rs"
                )
                ;;
            decode_stream)
                minimum_lines=50
                sources=(
                    "$PWD/crates/proxy-content/src/budget.rs"
                    "$PWD/crates/proxy-content/src/codec.rs"
                )
                ;;
            codec_roundtrip)
                minimum_lines=63
                sources=(
                    "$PWD/crates/proxy-content/src/budget.rs"
                    "$PWD/crates/proxy-content/src/codec.rs"
                    "$PWD/crates/proxy-content/src/coding.rs"
                )
                ;;
            content_pipeline)
                minimum_lines=50
                sources=(
                    "$PWD/crates/proxy-content/src/budget.rs"
                    "$PWD/crates/proxy-content/src/codec.rs"
                    "$PWD/crates/proxy-content/src/coding.rs"
                    "$PWD/crates/proxy-content/src/pipeline.rs"
                    "$PWD/crates/proxy-content/src/plan.rs"
                    "$PWD/crates/proxy-content/src/policy.rs"
                    "$PWD/crates/proxy-core/src/extensions.rs"
                    "$PWD/crates/proxy-core/src/header.rs"
                    "$PWD/crates/proxy-core/src/intercept/action.rs"
                    "$PWD/crates/proxy-core/src/intercept/body.rs"
                    "$PWD/crates/proxy-core/src/intercept/chain.rs"
                    "$PWD/crates/proxy-core/src/intercept/context.rs"
                    "$PWD/crates/proxy-core/src/intercept/mod.rs"
                    "$PWD/crates/proxy-core/src/task.rs"
                )
                ;;
            websocket_frames)
                minimum_lines=50
                sources=(
                    "$PWD/crates/proxy-websocket/src/compression.rs"
                    "$PWD/crates/proxy-websocket/src/frame.rs"
                )
                ;;
        esac
        printf '\n=== %s ===\n' "$target"
        coverage_report="$("$llvm_cov" report \
            "$target_dir/x86_64-unknown-linux-gnu/release/$target" \
            "-instr-profile=fuzz/coverage/$target/coverage.profdata" \
            "${sources[@]}")"
        printf '%s\n' "$coverage_report"
        line_coverage="$(awk '$1 == "TOTAL" { gsub("%", "", $10); print $10 }' <<<"$coverage_report")"
        if [[ -z "$line_coverage" ]]; then
            echo "Could not read $target line coverage." >&2
            exit 1
        fi
        if [[ -z "$minimum_lines" ]]; then
            printf 'Line coverage measurement: %s%% (no regression floor established yet)\n' "$line_coverage"
            continue
        fi
        if ! awk -v actual="$line_coverage" -v minimum="$minimum_lines" \
            'BEGIN { exit !(actual + 0 >= minimum + 0) }'; then
            echo "$target line coverage $line_coverage% is below the $minimum_lines% floor." >&2
            exit 1
        fi
        printf 'Line coverage gate: %s%% (minimum %s%%)\n' "$line_coverage" "$minimum_lines"
    done
} | tee "$summary"
