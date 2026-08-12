#!/usr/bin/env bash
# The name of the disk image, in one place.
#
# Sourced rather than run: dmg.sh builds the image and package.sh signs,
# notarizes and reports on it, so both need the name and neither owns it.
# Spelling it out in both is what broke the first Intel release build - dmg.sh
# wrote TinePlayer-1.2.0-dev-macos-arm64.dmg and package.sh went looking for
# TinePlayer-1.2.0-dev-macos.dmg, which is the same class of fault the release
# workflow already guards against by reading names off the artifacts.
#
# Callers must have cd'd to the repository root, which every script here does.

# Prints the path the disk image should have, relative to the repository root.
#
# The architecture is part of it because a release carries both and they are
# not interchangeable. uname -m is the source: it prints arm64 or x86_64,
# which is what macOS itself calls them, and it describes the machine that did
# the build rather than what a caller believed about it.
dmg_path() {
    local version
    version="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
    echo "dist/macos/TinePlayer-$version-macos-$(uname -m).dmg"
}
