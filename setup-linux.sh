#!/usr/bin/env bash
# Installs what TinePlayer needs to build and run on Linux:
#   - the Rust toolchain (via rustup, if not already installed)
#   - GTK 4 development headers, for the application window and for the
#     statically-linked gtk4paintablesink video sink
#   - GStreamer runtime + plugins for playback, including -ugly and -libav
#     for common Blu-ray-rip audio codecs like AC3 and DTS
#   - GStreamer development headers (including GL, required by
#     gtk4paintablesink), needed to build the Rust bindings from source
#
# Only what is needed to build. Adding a launcher entry for the copy you build
# is a separate step, in install-desktop.sh.
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

echo "Done. Next: cargo build --release && ./target/release/tineplayer"
