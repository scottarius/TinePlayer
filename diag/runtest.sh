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

# Deliberately not `eval "$(startenv.sh)"`: a command substitution swallows the
# exit status, so a failure to bring up the display sailed straight past `set
# -e` and every run afterwards reported healthy on an empty recording.
env_exports=$("$ROOT/diag/startenv.sh") || {
    echo "environment did not come up - refusing to report results" >&2
    exit 1
}
eval "$env_exports"

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

        # A run that recorded nothing is not a healthy run, and saying so is
        # the whole point: an empty recording produces no "silent from" line,
        # so without this check a dead environment reports as six clean passes.
        recorded=$(echo "$out" | grep -oE '[0-9]+s silent of [0-9]+s' |
            awk '{print $4}' | tr -d 's' | sort -rn | head -1)
        recorded=${recorded:-0}
        if [ "$recorded" -lt 10 ]; then
            echo "  run $i: NO DATA (recorded ${recorded}s - did it play at all?)"
        elif [ "$dead" -gt 0 ]; then
            echo "  run $i: FAILED ($dead output(s) died)  $summary"
        else
            echo "  run $i: healthy  $summary"
        fi
    done
done
