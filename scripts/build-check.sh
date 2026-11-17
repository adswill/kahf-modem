#!/usr/bin/env bash
# KAHF build check for macOS and Linux.
# Runs: cargo check --workspace, then cargo build -p kahf-cli.
# Usage: scripts/build-check.sh
# Logs go to a temporary folder whose path is printed at the end.

set -u

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO" || exit 1

LOG_DIR="$(mktemp -d "${TMPDIR:-/tmp}/kahf-build-check.XXXXXX")"
OS="$(uname -s)"
STEP_NAMES=()
STEP_RESULTS=()

say() { printf '%s\n' "$*"; }

if ! command -v cargo >/dev/null 2>&1; then
    say "ERROR: cargo was not found. Install Rust with rustup first (see docs/BUILD.md)."
    exit 2
fi

say "KAHF build check"
say "Host:      $OS $(uname -m)"
say "Repo:      $REPO"
say "Rust:      $(rustc --version 2>/dev/null || echo unknown)"
say "Logs:      $LOG_DIR"
say ""

# SDR library check. The kahf-sdr crate links hackrf and rtlsdr, so the build
# fails at link time if they are missing. cargo check does not link, so only
# the final build step needs them.
if [ "$OS" = "Darwin" ]; then
    for lib in hackrf rtlsdr; do
        if [ -e "/opt/homebrew/lib/lib$lib.dylib" ] || [ -e "/usr/local/lib/lib$lib.dylib" ]; then
            say "Found lib$lib (Homebrew)."
        else
            say "WARNING: lib$lib not found in /opt/homebrew/lib or /usr/local/lib."
            say "         Install it with: brew install $lib"
        fi
    done
elif [ "$OS" = "Linux" ]; then
    for lib in hackrf rtlsdr; do
        if ldconfig -p 2>/dev/null | grep -q "lib$lib\.so"; then
            say "Found lib$lib."
        else
            say "WARNING: lib$lib not found by ldconfig. The build step may fail to link."
            say "         Install the -dev package for $lib (see docs/BUILD.md)."
        fi
    done
fi
say ""

run_step() {
    local name="$1"
    shift
    local log="$LOG_DIR/$(echo "$name" | tr ' ' '_' | tr -d '-').log"
    say "==> $name"
    say "    $*"
    if "$@" >"$log" 2>&1; then
        STEP_NAMES+=("$name")
        STEP_RESULTS+=("PASS")
        say "    PASS"
    else
        STEP_NAMES+=("$name")
        STEP_RESULTS+=("FAIL")
        say "    FAIL (last lines of $log):"
        tail -n 15 "$log" | sed 's/^/      /'
    fi
    say ""
}

run_step "cargo check workspace" cargo check --workspace
run_step "cargo build kahf-cli" cargo build -p kahf-cli

say "Summary"
say "-------"
failed=0
for i in "${!STEP_NAMES[@]}"; do
    printf '  %-6s %s\n' "${STEP_RESULTS[$i]}" "${STEP_NAMES[$i]}"
    [ "${STEP_RESULTS[$i]}" = "PASS" ] || failed=1
done
say ""
if [ "$failed" -eq 0 ]; then
    say "Result: all steps passed on $OS $(uname -m)."
else
    say "Result: one or more steps failed. Full logs: $LOG_DIR"
fi
exit "$failed"
