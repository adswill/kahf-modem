#!/usr/bin/env bash
# Interop check: the kahf-winlink P2P link against the real wl2k-go library that Pat is built
# on, both directions, with an attachment and non-ASCII text. Needs Go and network access to
# fetch wl2k-go (MIT). Not part of CI.
set -euo pipefail
cd "$(dirname "$0")/.."
WORK=$(mktemp -d)
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$WORK"' EXIT

git clone -q --depth 1 --branch v1.0.1 https://github.com/la5nta/wl2k-go "$WORK/wl2k-go"
mkdir -p "$WORK/wl2k-go/cmd/interop"
cp crates/kahf-winlink/interop/main.go "$WORK/wl2k-go/cmd/interop/"
(cd "$WORK/wl2k-go" && go build -o "$WORK/interop" ./cmd/interop)
cargo build -q -p kahf-winlink
W=target/debug/kahf-winlink

echo "café ☕ from kahf" >"$WORK/body.txt"
head -c 3000 /dev/urandom >"$WORK/att.bin"

for scenario in rust-listens go-listens; do
  R="$WORK/$scenario/rust"; G="$WORK/$scenario/go"; mkdir -p "$R" "$G"
  $W send --root "$R" --from K1ABC --to N0CALL --subject "$scenario" --body "$WORK/body.txt" --attach "$WORK/att.bin" >/dev/null
  $W send --root "$G" --from N0CALL --to K1ABC --subject "reply $scenario" --body "$WORK/body.txt" >/dev/null
  if [ "$scenario" = rust-listens ]; then
    $W listen --call K1ABC --root "$R" --bind 127.0.0.1:18774 >/dev/null &
    sleep 1
    "$WORK/interop" client 127.0.0.1:18774 N0CALL K1ABC "$G/N0CALL"
  else
    "$WORK/interop" server 127.0.0.1:18775 N0CALL "$G/N0CALL" &
    sleep 1
    $W call 127.0.0.1:18775 --call K1ABC --target N0CALL --root "$R"
    wait %2 || true
  fi
  python3 - "$G/N0CALL/in" "$WORK/att.bin" <<'PY'
import sys, pathlib
inbox, att = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]).read_bytes()
files = list(inbox.glob("*.b2f"))
assert len(files) == 1 and att in files[0].read_bytes(), "attachment missing or damaged"
PY
  [ "$(ls "$R/K1ABC/in" | wc -l)" = 1 ] || { echo "FAIL: $scenario: nothing arrived at kahf"; exit 1; }
  echo "PASS: $scenario"
done
