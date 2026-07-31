#!/usr/bin/env bash
#
# Offers TinePlayer to Kodi as an external player.
#
# Writes playercorefactory.xml into Kodi's userdata directory. Kodi has no
# interface for this, so it is otherwise a hand-edited file.
#
# By default TinePlayer appears under "Play using..." in a video's context menu
# and Kodi goes on playing videos itself. Pass --default to send every video to
# TinePlayer instead.
#
# An existing playercorefactory.xml is backed up rather than replaced silently,
# since it may configure other players.
#
# Usage: ./install-kodi.sh [--default] [--userdata DIR]

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
as_default=false
userdata=""

while [ $# -gt 0 ]; do
    case "$1" in
        --default) as_default=true ;;
        --userdata) userdata="$2"; shift ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
    shift
done

binary="$root/target/release/TinePlayer"
if [ ! -x "$binary" ]; then
    echo "TinePlayer not found at $binary. Build it first with: cargo build --release" >&2
    exit 1
fi

# Covers a normal install, Flatpak, snap, and the settop builds.
if [ -z "$userdata" ]; then
    for candidate in \
        "$HOME/.kodi/userdata" \
        "$HOME/.var/app/tv.kodi.Kodi/data/userdata" \
        "$HOME/snap/kodi/current/.kodi/userdata" \
        "/storage/.kodi/userdata"
    do
        if [ -d "$candidate" ]; then
            userdata="$candidate"
            break
        fi
    done
fi
if [ -z "$userdata" ]; then
    echo "Could not find Kodi's userdata directory. Pass --userdata with its location." >&2
    exit 1
fi

target="$userdata/playercorefactory.xml"
xml="$(sed "s|TINEPLAYER_BINARY|$binary|" "$root/data/playercorefactory.xml")"

if [ "$as_default" = true ]; then
    xml="$(printf '%s\n' "$xml" | sed '/<!-- RULES \(START\|END\) -->/d')"
else
    # Drop the rules block, leaving TinePlayer selectable rather than forced.
    xml="$(printf '%s\n' "$xml" | sed '/<!-- RULES START -->/,/<!-- RULES END -->/d')"
fi

if [ -f "$target" ]; then
    backup="$target.$(date +%Y%m%d-%H%M%S).bak"
    cp "$target" "$backup"
    echo "Existing file backed up to $backup"
    echo "If it configured other players, merge them back by hand."
fi

printf '%s\n' "$xml" > "$target"
echo "Wrote $target"
if [ "$as_default" = true ]; then
    echo "Kodi will now play all video through TinePlayer."
else
    echo "In Kodi, use \"Play using...\" on a video to choose TinePlayer."
fi
echo "Restart Kodi for it to notice."
