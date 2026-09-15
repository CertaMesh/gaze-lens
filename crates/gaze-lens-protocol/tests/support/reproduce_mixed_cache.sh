#!/usr/bin/env bash
# Exercise actual Cargo linkage across broken/fixed sources and cached flags.
set -euo pipefail
root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
proof=${1:-$(mktemp -d "${TMPDIR:-/tmp}/lens-allocation-cache.XXXXXX")}
mkdir -p "$proof"
proof=$(cd "$proof" && pwd)
if [[ -e "$proof/parent" || -e "$proof/corrected" || -e "$proof/target" ]]; then
    echo 'Use an unused proof directory; existing caches are never removed.' >&2
    exit 1
fi
parent=6963affb4d8840897097b53af6865397b220c5ad
corrected=6a4fe7982692eacc9b23f0f3aa2382c6213ba959
package=crates/gaze-lens-protocol
for revision in parent corrected; do
    mkdir "$proof/$revision"
    git -C "$root" archive "${!revision}" | tar -x -C "$proof/$revision"
    mkdir -p "$proof/$revision/$package/tests/support"
    # Apply only the current test/build wiring to the exact production sources.
    for file in Cargo.toml build.rs tests/support/allocation_probe.rs tests/support/allocation_meter.rs; do
        cp "$root/$package/$file" "$proof/$revision/$package/$file"
    done
    rm -f "$proof/$revision/$package/tests/allocation_admission.rs"
done
export CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$proof/target"
# Explicitly exercise normal and alternate flags, independent of shell flags.
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
run() {
    local label=$1 source=$2 flags=$3 expected=$4 status=0
    (cd "$proof/$source" && RUSTFLAGS="$flags" cargo test \
        -p gaze-lens-protocol --test allocation_admission --locked --offline) \
        > "$proof/$label.log" 2>&1 || status=$?
    printf '%s exit=%s expected=%s\n' "$label" "$status" "$expected" | tee -a "$proof/results.txt"
    test "$status" -eq "$expected"
    case "$label" in
        parent-mixed|*-cached)
            # Retest the retained binary, as in the original false-green report.
            if grep -F 'Compiling gaze-lens-protocol' "$proof/$label.log"; then
                echo 'Expected the existing Cargo build to be reused.' >&2
                exit 1
            fi
            ;;
    esac
    if [[ "$expected" == 101 ]]; then
        # A compiler error is not a successful negative allocation control.
        grep -F 'allocation requests: (49000, 49000)' "$proof/$label.log"
    else
        grep -F 'allocation admission controls passed' "$proof/$label.log"
    fi
}
run parent-control parent '' 101
run corrected-alternate corrected '-C debuginfo=1' 0
run parent-mixed parent '' 101
run corrected-alternate-cached corrected '-C debuginfo=1' 0
run parent-mixed-cached parent '' 101
printf 'Mixed-cache proof retained at %s\n' "$proof"
