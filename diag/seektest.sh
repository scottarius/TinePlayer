#!/bin/bash
# SCAFFOLDING - branch fix/linux-seek-audio only. This directory and the
# TINEPLAYER_SEEK_TEST / TINEPLAYER_APP_ID / TINEPLAYER_NO_SEEK_WORKAROUND
# hooks it drives must not reach main: the fix goes over on its own.
#
# Reproduces the Linux two-output seek bug and measures it by recording what
# actually reaches each device, which is the only measurement that has ever
# agreed with what a person hears. Four in-process metrics reported healthy
# while the audio was audibly gone, so nothing here trusts the pipeline's own
# account of itself.
#
#   DURATION=60 EVERY=8 ./seektest.sh
#
# Two null sinks stand in for real devices: their monitors are recordable, and
# the fault was already shown not to be about the devices - pointing the second
# branch at a fakesink failed identically.
set -u

DUR=${DURATION:-60}
EVERY=${EVERY:-8}
VIDEO=${VIDEO:-/mnt/hoth/Videos/Movies/Avengers - Endgame (2019)/Avengers - Endgame (2019) Bluray-1080p.mkv}
PRIMARY=${PRIMARY:-1}
SECONDARY=${SECONDARY:-2}

export XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-0
export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus

# A stray instance holding the same devices produces audio symptoms of its own,
# so nothing else may be running.
pkill -x TinePlayer 2>/dev/null
sleep 1

# The diagnostic run gets its own config, so the real one is never touched and
# a resume position from an earlier run cannot move the starting point.
DIAG=/tmp/tpdiag
rm -rf "$DIAG"
mkdir -p "$DIAG/config/tineplayer" "$DIAG/data" "$DIAG/state"
export XDG_CONFIG_HOME=$DIAG/config XDG_DATA_HOME=$DIAG/data XDG_STATE_HOME=$DIAG/state
# The sink is matched on its *display name*, which for a PipeWire sink is its
# description rather than its node name - so TP_A here and tp_a below. Getting
# this wrong is silent: an unmatched name leaves the primary output
# unconfigured, and an unconfigured primary means the run lands on the menu and
# never plays at all.
cat > "$DIAG/config/tineplayer/config.yaml" <<'YAML'
primary_sink: TP_A
secondary_sink: TP_B
YAML

for s in tp_a tp_b; do
    pactl list short sinks | grep -q "	$s	" || {
        echo "missing sink $s - create it with:"
        echo "  pactl load-module module-null-sink sink_name=$s"
        exit 1
    }
done

# What PulseAudio thinks of our two streams, sampled from outside the process.
# In-process metrics are what misled this investigation twice, so the state of
# the stream is asked of the sound server rather than of the sink element.
rm -f /tmp/sinkinputs.log
(
    while true; do
        printf '@%s ' "$(date +%s)"
        pactl list sink-inputs |
            tr '\n' '|' |
            sed 's/Sink Input #/\nSink Input #/g' |
            grep -oE 'Sink Input #[0-9]+.*' |
            sed -E 's/.*Sink Input #([0-9]+).*Sink: ([0-9]+).*Corked: (\w+).*Mute: (\w+).*/in=\1 sink=\2 corked=\3 mute=\4/' |
            tr '\n' ' '
        echo
        sleep 1
    done
) > /tmp/sinkinputs.log 2>&1 &
PS_PID=$!

rm -f /tmp/tp_a.wav /tmp/tp_b.wav
parec -d tp_a.monitor --file-format=wav --rate=16000 --channels=1 /tmp/tp_a.wav & PA=$!
parec -d tp_b.monitor --file-format=wav --rate=16000 --channels=1 /tmp/tp_b.wav & PB=$!
REC_START=$(date +%s.%N)

TINEPLAYER_SEEK_TEST=$EVERY \
TINEPLAYER_APP_ID=app.tineplayer.Diag \
TINEPLAYER_SEEK_PROBE=${PROBE:-0} \
    timeout "$DUR" /mnt/hoth/TinePlayer/target/release/TinePlayer "$VIDEO" \
    --primary "$PRIMARY" --secondary "$SECONDARY" --restart --play \
    > /tmp/tp.log 2>&1

kill $PA $PB $PS_PID 2>/dev/null
sleep 1

echo
echo "=== what PulseAudio saw of our streams, once per second ==="
echo "    t is seconds from the start of the recording, so these line up with"
echo "    the timeline below - without a shared clock there is no telling"
echo "    whether a cork preceded the silence or followed it"
awk -v base="$REC_START" '{t=substr($1,2)-base; $1=""; if (t >= -1) printf "  t=%-4d%s\n", t, $0}' /tmp/sinkinputs.log

echo
echo "=== seeks issued, on the recording's clock ==="
# Three-argument match() is a gawk extension and the Pi has mawk, where it
# fails quietly and the whole alignment is lost. Plain field access instead.
awk -v base="$REC_START" '/^DIAG: seek/ {
    wall = $NF; gsub(/[()]/, "", wall)
    printf "  seek %s at t=%.1f\n", $3, wall - base
}' /tmp/tp.log
if [[ ${PROBE:-0} == 1 ]]; then
    echo
    echo "=== buffers per second, and flush events (PROBE=1) ==="
    echo "    where the counts stop is where data stopped; whether sound"
    echo "    reached a device is decided by the recording below, not here"
    grep -E '^(PROBE|DIAG)' /tmp/tp.log
fi

echo
echo "=== errors ==="
grep -iE 'error|warn|fail|silent' /tmp/tp.log | head -20 || true
echo

REC_START=$REC_START python3 - <<'PY'
import os, wave, audioop, warnings
warnings.filterwarnings('ignore')

# One character per second: '#' audible, '.' faint, ' ' silent. Printed against
# a second scale so a gap can be read straight off against the seek log above.
for name, path in (('tp_a (primary)  ', '/tmp/tp_a.wav'),
                   ('tp_b (secondary)', '/tmp/tp_b.wav')):
    try:
        w = wave.open(path)
    except Exception as e:
        print(f'{name} unreadable: {e}')
        continue
    rate, n = w.getframerate(), w.getnframes()
    row, silent = [], 0
    for s in range(int(n / rate)):
        w.setpos(s * rate)
        rms = audioop.rms(w.readframes(rate), w.getsampwidth())
        row.append('#' if rms > 300 else ('.' if rms > 30 else ' '))
        if rms <= 30:
            silent += 1
    total = len(row)
    print(f'{name} |{"".join(row)}|  {silent}s silent of {total}s')

    # Where it went quiet and stayed quiet, which is the shape of this bug as
    # opposed to an ordinary gap while a seek settles.
    tail = 0
    for c in reversed(row):
        if c != ' ':
            break
        tail += 1
    if tail >= 5:
        print(f'{" " * len(name)}  ^ silent from {total - tail}s to the end')

scale = ''.join(str((i // 10) % 10) if i % 10 == 0 else ' ' for i in range(200))
print(f'{" " * 17} |{scale[:200]}|  (tens of seconds)')
PY
