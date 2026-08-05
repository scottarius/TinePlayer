#!/bin/bash
# SCAFFOLDING - branch fix/linux-seek-audio only.
#
# Prepares a Debian or Ubuntu machine to run the seek harness: build
# dependencies, a GStreamer stack, PipeWire, and a compositor. Written to be
# run on a bare VM, so it assumes nothing is present and is safe to re-run.
#
# The point of these environments is to move one variable - the GStreamer
# version - against the Pi's 1.22.0, and find out whether the seek fault is
# that stack or that machine.
set -eu

echo "=== $(. /etc/os-release && echo "$PRETTY_NAME") on $(uname -m) ==="

sudo apt-get update -qq
# gtk4paintablesink comes from a Rust crate rather than a distro package, so
# there is no gstreamer1.0-gtk4 here on purpose.
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential pkg-config curl ca-certificates git python3 ffmpeg \
    libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
    gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav \
    gstreamer1.0-pulseaudio gstreamer1.0-tools \
    pipewire pipewire-pulse wireplumber pulseaudio-utils \
    weston libxkbcommon0 >/dev/null

if [ ! -x "$HOME/.cargo/bin/cargo" ] && ! command -v cargo >/dev/null; then
    echo "=== installing rust ==="
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
        sh -s -- -y --profile minimal --no-modify-path >/dev/null
fi

echo
echo "=== versions that matter ==="
gst-inspect-1.0 --version | sed -n 2p
pkg-config --modversion gtk4 | sed 's/^/GTK /'
pipewire --version 2>&1 | sed -n 2p
"$HOME/.cargo/bin/cargo" --version 2>/dev/null || cargo --version
