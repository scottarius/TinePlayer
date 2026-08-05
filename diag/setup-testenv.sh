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

# WSL's default user needs a password for sudo where the Pi and the Lima VMs
# do not, and a password prompt with no stdin waits for ever rather than
# failing - the setup simply sits there looking busy. So: no sudo when already
# root, and a refusal up front rather than a hang when it would block.
SUDO=sudo
if [ "$(id -u)" = 0 ]; then
    SUDO=
elif ! sudo -n true 2>/dev/null; then
    echo "sudo needs a password here. Run this as root instead:" >&2
    echo "  wsl.exe -d Ubuntu -u root -- $0" >&2
    exit 1
fi

$SUDO apt-get update -qq
# gtk4paintablesink comes from a Rust crate rather than a distro package, so
# there is no gstreamer1.0-gtk4 here on purpose.
# `env` rather than a bare assignment prefix: bash decides at parse time
# whether a word is an assignment, and with $SUDO empty the prefix is already
# an argument by then - "DEBIAN_FRONTEND=noninteractive: command not found".
$SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
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
