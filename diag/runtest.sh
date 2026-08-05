#!/bin/bash
# SCAFFOLDING - branch fix/linux-seek-audio only.
#
# One command per environment: bring up the session, build, make a clip, and
# run the seek test both ways. Run from the source tree.
#
#   ./diag/runtest.sh [runs]
#
# Reports each run as healthy or failed so environments can be compared
# without reading six timelines by eye. "Failed" means an output went silent
# and stayed silent to the end, which is the fault under investigation - not
# the ordinary short gap while a seek settles.
set -eu

RUNS="${1:-3}"
cd "$(dirname "$0")/.."
ROOT="$PWD"

eval "$("$ROOT/diag/startenv.sh")"

CARGO="$HOME/.cargo/bin/cargo"
command -v cargo >/dev/null && CARGO=cargo
echo "=== building ==="
"$CARGO" build --release 2>&1 | tail -3

CLIP=/tmp/seekclip.mkv
[ -f "$CLIP" ] || "$ROOT/diag/mkclip.sh" "$CLIP"

export BIN="$ROOT/target/release/TinePlayer"
export VIDEO="$CLIP"

echo
echo "=== $(. /etc/os-release && echo "$PRETTY_NAME") $(uname -m) ==="
echo "=== GStreamer $(gst-inspect-1.0 --version | sed -n 2p | awk '{print $2}') ==="

for mode in "workaround off:1" "workaround on:"; do
    label="${mode%%:*}"
    value="${mode##*:}"
    echo
    echo "--- $label ---"
    for i in $(seq "$RUNS"); do
        if [ -n "$value" ]; then
            export TINEPLAYER_NO_SEEK_WORKAROUND=1
        else
            unset TINEPLAYER_NO_SEEK_WORKAROUND
        fi
        out=$(DURATION=40 EVERY=8 "$ROOT/diag/seektest.sh" 2>&1 || true)
        dead=$(echo "$out" | grep -c "silent from" || true)
        summary=$(echo "$out" | grep -E "^tp_[ab] " | sed 's/  */ /g' | tr '\n' '|')
        if [ "$dead" -gt 0 ]; then
            echo "  run $i: FAILED ($dead output(s) died)  $summary"
        else
            echo "  run $i: healthy  $summary"
        fi
    done
done
