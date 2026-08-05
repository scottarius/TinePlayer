#!/bin/bash
# SCAFFOLDING - branch fix/linux-seek-audio only.
#
# Builds the clip the seek harness plays. Two audio tracks, each a continuous
# tone at its own pitch, so a recording of a null sink shows both *that* audio
# is flowing and *which* track it is.
#
# Continuous rather than the sample film's beeps: this measures how long an
# output stays silent, and a track that is silent between beeps by design
# cannot answer that. The sample film is right for screenshots and for proving
# routing, and wrong for this.
#
#   ./mkclip.sh [output.mkv]
set -eu

OUT="${1:-/tmp/seekclip.mkv}"
DUR="${DUR:-300}"

command -v ffmpeg >/dev/null || {
    echo "ffmpeg not found" >&2
    exit 1
}

# 720p and a fast preset: the video only has to decode like a real file, and
# nothing here looks at the picture. Keyframes every two seconds so an ACCURATE
# seek has somewhere near to land without decoding half the file.
ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -t "$DUR" -i "testsrc2=size=1280x720:rate=25" \
    -f lavfi -t "$DUR" -i "sine=frequency=440:sample_rate=48000" \
    -f lavfi -t "$DUR" -i "sine=frequency=587:sample_rate=48000" \
    -map 0:v -map 1:a -map 2:a \
    -c:v libx264 -preset veryfast -crf 30 -g 50 -pix_fmt yuv420p \
    -c:a aac -b:a 128k \
    -metadata:s:a:0 language=eng -metadata:s:a:0 title="Tone A 440" \
    -metadata:s:a:1 language=spa -metadata:s:a:1 title="Tone B 587" \
    "$OUT"

echo "built $OUT"
ffprobe -v error -select_streams a -show_entries stream=index,codec_name:stream_tags=title \
    -of csv=p=0 "$OUT"
