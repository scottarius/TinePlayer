#!/usr/bin/env bash
# Installs what TinePlayer needs to build and run on Linux:
#   - the Rust toolchain (via rustup, if not already installed)
#   - GTK 4 development headers, for the application window and for the
#     statically-linked gtk4paintablesink video sink
#   - GStreamer runtime + plugins for playback, including -ugly and -libav
#     for common Blu-ray-rip audio codecs like AC3 and DTS
#   - GStreamer development headers (including GL, required by
#     gtk4paintablesink), needed to build the Rust bindings from source
set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
fi

sudo apt update
sudo apt install -y \
    pkg-config \
    libgtk-4-dev \
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev \
    gstreamer1.0-tools \
    gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad \
    gstreamer1.0-plugins-ugly \
    gstreamer1.0-libav \
    alsa-utils

# --- Desktop integration ----------------------------------------------
# Gives the application an icon and a launcher entry. The Exec line points at
# the built binary rather than assuming it is on PATH, since nothing here
# installs it system-wide. Wayland matches a running window to this file by
# the application id, which is what puts the icon in the taskbar too.
icons="$HOME/.local/share/icons/hicolor/scalable/apps"
applications="$HOME/.local/share/applications"
mkdir -p "$icons" "$applications"

cp data/dev.tineplayer.TinePlayer.svg "$icons/"
sed "s|^Exec=.*|Exec=$PWD/target/release/TinePlayer %f|"     data/dev.tineplayer.TinePlayer.desktop     > "$applications/dev.tineplayer.TinePlayer.desktop"

# Both are best-effort: the desktop still works without the caches, they just
# take longer to notice the new entry.
update-desktop-database "$applications" 2>/dev/null || true
gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

echo "Done. Next: cargo build --release && ./target/release/TinePlayer"
