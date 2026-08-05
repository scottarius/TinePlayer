#!/usr/bin/env bash
# Installs what TinePlayer needs to build and run on macOS:
#   - Homebrew, which also installs the Xcode Command Line Tools if they are
#     missing, so there is nothing to install by hand beforehand
#   - the Rust toolchain (via rustup, if not already installed)
#   - GTK 4, GStreamer and its plugins, and pkg-config
#
# Everything comes from Homebrew on purpose. GStreamer's own macOS package
# ships no GTK, so GTK would have to come from elsewhere anyway, and two
# sources means two copies of glib - the same conflict that breaks the build on
# Windows when GTK is installed separately. One package manager, one glib.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "This script is for macOS. On Linux use ./install.sh." >&2
    exit 1
fi

# Homebrew lives in /opt/homebrew on Apple Silicon and /usr/local on Intel, and
# is on PATH in neither a fresh install nor a non-login shell. Looked for by
# path before deciding whether it is installed at all: asking `command -v`
# first would call it missing on a machine that has it, and then try to install
# it again.
put_brew_on_path() {
    for prefix in /opt/homebrew /usr/local; do
        if [[ -x "$prefix/bin/brew" ]]; then
            eval "$("$prefix/bin/brew" shellenv)"
            return 0
        fi
    done
    return 1
}

if ! put_brew_on_path; then
    # Homebrew asks for a password, and macOS will not prompt for one without a
    # terminal attached. Piping this script, or running it over a
    # non-interactive ssh command, otherwise fails deep inside the installer
    # with a message about needing an Administrator rather than a terminal.
    if [[ ! -t 0 ]]; then
        echo "Homebrew is not installed, and installing it needs your password." >&2
        echo "Run this from a terminal rather than through a pipe or ssh command." >&2
        exit 1
    fi

    echo "Installing Homebrew (this also installs the Xcode Command Line Tools)..."
    NONINTERACTIVE=1 /bin/bash -c \
        "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

    if ! put_brew_on_path; then
        echo "Homebrew installed but could not be found on PATH." >&2
        exit 1
    fi
fi

if ! command -v cargo >/dev/null 2>&1 && [[ ! -x "$HOME/.cargo/bin/cargo" ]]; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
fi
# shellcheck disable=SC1091
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"

# The gstreamer formula brings the plugin sets with it, including libav, which
# is what decodes the AC-3 and DTS tracks common in Blu-ray rips. gtk4 brings
# glib, pango and cairo. Nothing else is needed to build the bindings.
brew install gtk4 gstreamer pkg-config

# Homebrew is not on PATH in a new shell until this is in the profile, and the
# build needs it there to find pkg-config.
profile="$HOME/.zprofile"
[[ "${SHELL##*/}" == "bash" ]] && profile="$HOME/.bash_profile"
if ! grep -q "brew shellenv" "$profile" 2>/dev/null; then
    echo "" >>"$profile"
    echo "eval \"\$($(command -v brew) shellenv)\"" >>"$profile"
    echo "Added Homebrew to $profile."
fi

echo
echo "Done. Open a new terminal, then:"
echo "    cargo build --release"
echo "    ./target/release/tineplayer"
