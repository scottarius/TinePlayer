#!/usr/bin/env bash
# Adds a launcher entry and icon for the copy of TinePlayer you have built, so
# it can be started from the desktop rather than only from a terminal.
#
# Separate from setup-linux.sh, which only installs what is needed to build:
# this puts something in your home directory, and wanting to build is not the
# same as wanting a launcher.
#
# The entry points at the binary in this working tree, so it stops working if
# the tree moves or is deleted. Run this again from the new location to fix it.
# A packaged build will carry its own entry and need none of this.
set -euo pipefail

cd "$(dirname "$0")"

binary="$PWD/target/release/TinePlayer"
if [[ ! -x "$binary" ]]; then
    echo "No release build found. Run: cargo build --release" >&2
    exit 1
fi

icons="$HOME/.local/share/icons/hicolor/scalable/apps"
applications="$HOME/.local/share/applications"
mkdir -p "$icons" "$applications"

cp data/branding/app.tineplayer.TinePlayer.svg "$icons/"
# The Exec line is rewritten rather than assuming the binary is on PATH, since
# nothing here installs it system-wide. Wayland matches a running window to
# this file by the application id, which is also what puts the icon in the
# taskbar.
sed "s|^Exec=.*|Exec=$binary %f|" \
    data/templates/app.tineplayer.TinePlayer.desktop \
    >"$applications/app.tineplayer.TinePlayer.desktop"

# Both are best-effort: the desktop still works without the caches, they just
# take longer to notice the new entry.
update-desktop-database "$applications" 2>/dev/null || true
gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

echo "Added a launcher entry for $binary"
