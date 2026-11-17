#!/usr/bin/env bash
# End-to-end chain check: random file -> TX WAV -> channel profile -> RX WAV -> compare.
# Runs `kahf-cli e2e` for every profile and SNR, one line per run, then a summary.
#
# Usage:  tests/e2e/run_all.sh
# Env:    PROFILES  (default: awgn two-path freq-offset clock-offset fade)
#         SNRS      (default: 20 15 10)
#         BYTES     (default: 1000)
# Quick check:  SNRS=20 tests/e2e/run_all.sh
set -u

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT" || exit 2

PROFILES="${PROFILES:-awgn two-path freq-offset clock-offset fade}"
SNRS="${SNRS:-20 15 10}"
BYTES="${BYTES:-1000}"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"

if ! cargo build -q -p kahf-cli; then
    echo "build failed"
    exit 2
fi
BIN="$TARGET/debug/kahf-cli"

pass=0
fail=0
for p in $PROFILES; do
    for s in $SNRS; do
        line="$("$BIN" e2e --profile "$p" --snr "$s" --bytes "$BYTES" 2>&1)"
        rc=$?
        echo "$line"
        if [ "$rc" -eq 0 ]; then
            pass=$((pass + 1))
        else
            fail=$((fail + 1))
        fi
    done
done

echo "summary: PASS=$pass FAIL=$fail"
[ "$fail" -eq 0 ]
