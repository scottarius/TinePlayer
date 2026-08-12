#!/usr/bin/env bash
# Wraps the finished bundle in a disk image: the window with the application
# on the left, a shortcut to Applications on the right, and an arrow between
# them. It is what a Mac user expects, and it means nobody has to be told
# where an application goes.
#
# Run after ./packaging/macos/package.sh, which is what fills the bundle.
set -euo pipefail

cd "$(dirname "$0")/../.."

app="dist/macos/TinePlayer.app"
if [[ ! -d "$app" ]]; then
    echo "No bundle found. Run: ./packaging/macos/package.sh" >&2
    exit 1
fi

export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

# Every test build mounts itself and stays mounted, so the next one arrives
# as "TinePlayer 1", the one after as "TinePlayer 2", and what is on screen
# is whichever old volume was opened first. Clearing them up front is the
# difference between testing this build and testing one from an hour ago.
for volume in /Volumes/TinePlayer*; do
    [[ -d "$volume" ]] || continue
    hdiutil detach "$volume" -force -quiet 2>/dev/null && echo "Ejected stale $volume"
done

version="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
out="dist/macos"
# Named for the architecture it was built on, because the two packages are not
# interchangeable and the release carries both. uname -m is the right source
# for the name: it prints arm64 or x86_64, which is what macOS itself calls
# them, and it describes the machine that actually did the build rather than
# what a caller believed about it.
dmg="$out/TinePlayer-$version-macos-$(uname -m).dmg"
mkdir -p "$out"
rm -f "$dmg"

# Built with dmgbuild, which writes the window layout into the image's own
# .DS_Store from Python. create-dmg produces the same thing by driving Finder
# with AppleScript, which needs a logged-in graphical session: over SSH it
# times out, and on a build runner there is no session at all.
#
# Installed into a virtualenv under target/ rather than onto the machine, so
# packaging leaves nothing behind and a build runner gets the same versions.
venv="target/packaging-cache/dmg-venv"
if [[ ! -x "$venv/bin/dmgbuild" ]]; then
    echo "Installing dmgbuild..."
    python3 -m venv "$venv"
    "$venv/bin/pip" install --quiet --upgrade pip
    "$venv/bin/pip" install --quiet dmgbuild
fi

# --- Background ---------------------------------------------------------
#
# Supply one image at twice the window size and this makes the rest. A disk
# image wants both resolutions in a single TIFF - the ordinary one and the
# retina one - marked so macOS knows the second is the same picture at higher
# density rather than a second page. sips and tiffutil both ship with macOS,
# so this needs nothing installed.
art_png="packaging/macos/dmg-background.png"
art_tiff="packaging/macos/dmg-background.tiff"
built_tiff="target/packaging-cache/dmg-background.tiff"

if [[ -f "$art_tiff" ]]; then
    # A TIFF made by hand wins: somebody who built one knows what they want.
    export TINE_DMG_BACKGROUND="$art_tiff"
elif [[ -f "$art_png" ]]; then
    echo "Building the retina background..."
    mkdir -p "$(dirname "$built_tiff")"
    tmp="$(mktemp -d)"
    # The supplied image is the 2x one; the 1x is it at half size.
    sips --resampleHeightWidth 400 660 "$art_png" --out "$tmp/1x.png" >/dev/null
    cp "$art_png" "$tmp/2x.png"
    tiffutil -cathidpicheck "$tmp/1x.png" "$tmp/2x.png" -out "$built_tiff" >/dev/null
    rm -rf "$tmp"
    export TINE_DMG_BACKGROUND="$built_tiff"
fi

# Hide the .app extension on the icon in the window.
#
# dmgbuild's hide_extensions setting does not do it: Finder reads the flag
# from the bundle's own com.apple.FinderInfo, not from the .DS_Store. That
# attribute is 32 bytes, and the flags live in bytes 8 and 9 - 0x0010 is
# kIsExtensionHidden. Set here rather than in bundle.sh so it holds however
# the bundle was produced, and it survives the copy into the image.
xattr -wx com.apple.FinderInfo "0000000000000000001000000000000000000000000000000000000000000000" "$app"

echo "Building the disk image..."
TINE_APP="$app" "$venv/bin/dmgbuild"     -s packaging/macos/dmg-settings.py     "TinePlayer $version"     "$dmg"

size="$(du -h "$dmg" | cut -f1)"
echo
echo "Built $dmg ($size)"
# Only when nothing is going to sign it. package.sh runs this a second time
# inside the signing block, where the image is about to be signed and
# notarized, and saying it is unsigned there is simply wrong.
if [[ -z "${TINE_SIGN_IDENTITY:-}" ]]; then
    echo
    echo "Unsigned, so a copy downloaded from the internet is quarantined until"
    echo "it is notarized. On the machine that built it, it opens as it is."
fi
