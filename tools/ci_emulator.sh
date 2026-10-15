#!/usr/bin/env bash
# End-to-end test of the serial layer against ELM327-emulator with the made-up
# Ford scenario (tools/ford_emulator.py). Used by CI; can also be run locally:
#
#   pip install ELM327-emulator   # or in a venv
#   tools/ci_emulator.sh target/release/fordiag
#
# The emulator data is made up; this only checks that fordiag talks correctly
# over a real (pseudo-)serial port: baud probe, init, targeting, multi-frame
# answers, negative answers and the MS-CAN setup.

set -euo pipefail

FORDIAG="${1:-target/release/fordiag}"
PYTHON="${PYTHON:-python3}"
WORK="$(mktemp -d)"
EMU_OUT="$WORK/emulator.out"

cleanup() {
    if [[ -n "${EMU_PID:-}" ]]; then kill "$EMU_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

# Batch mode: the emulator reads commands from stdin (none here) and writes the
# pseudo-terminal name as the first line of its output.
"$PYTHON" tools/ford_emulator.py -b - < /dev/null > "$EMU_OUT" 2>&1 &
EMU_PID=$!

PORT=""
for _ in $(seq 1 50); do
    PORT="$(grep -o '/dev/pts/[0-9]*' "$EMU_OUT" | head -1 || true)"
    [[ -n "$PORT" ]] && break
    sleep 0.2
done
if [[ -z "$PORT" ]]; then
    echo "emulator did not start:"; cat "$EMU_OUT"; exit 1
fi
echo "emulator on $PORT"

fail() { echo "FAIL: $*"; exit 1; }

run() {
    echo "+ fordiag $*"
    "$FORDIAG" --port "$PORT" --data-dir "$WORK/data" -y "$@" | tee "$WORK/last.txt"
}

run connect --probe
grep -q "ELM version: ELM327" "$WORK/last.txt" || fail "connect: no ELM version"
grep -q "7E8 06 41 00" "$WORK/last.txt" || fail "connect: no answer to 01 00"

run dtc --bus hs
grep -q "Responded on HS-CAN: PCM, IPC, BCM, ACM, APIM" "$WORK/last.txt" || fail "dtc hs: unexpected modules"
grep -q "U3003:16" "$WORK/last.txt" || fail "dtc hs: multi-frame BCM answer not decoded"
grep -q "negative response 0x31" "$WORK/last.txt" || fail "dtc hs: ACM negative answer not decoded"
grep -q "P0563" "$WORK/last.txt" || fail "dtc hs: OBD pending DTC not decoded"

run dtc --bus ms
grep -q "Responded on MS-CAN: PCM, IPC, BCM, ACM, APIM" "$WORK/last.txt" || fail "dtc ms: unexpected modules"

# Everything that was sent must be on the allowlist.
for wire in $(grep -ho '"dir":"tx","data":"[^"]*"' "$WORK"/data/logs/*.jsonl | sed 's/.*"data":"//; s/"$//' | sort -u); do
    "$FORDIAG" check "$wire" | grep -q '^ALLOWED' || fail "sent a command that is not allowed: $wire"
done

"$FORDIAG" --data-dir "$WORK/data" report --no-raw -o "$WORK/report.md" > /dev/null
grep -q "^## DTCs" "$WORK/report.md" || fail "report: no DTC section"

echo "emulator end-to-end test passed"
