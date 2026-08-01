#!/usr/bin/env bash
# Wraps the built binary in a TinePlayer.app bundle.
#
# macOS has no other way to make a program double-clickable. Finder hands a
# bare Unix executable to Terminal.app, which runs it in a shell and leaves the
# window sitting there after the application quits. A bundle goes through
# LaunchServices instead: no terminal, a real Dock icon, and the application's
# own name in the menu bar.
#
# This is the counterpart to the .desktop file install.sh writes on Linux.
#
# The bundle links against the Homebrew copies of GTK and GStreamer, so it runs
# on a machine set up by install-mac.sh and not on one without them. Making it
# self-contained means copying those libraries in and rewriting their install
# names, which belongs with the rest of the packaging work.
#
# Only the wrapper. Making the result run on a Mac without Homebrew is a
# different job, and lives in package-mac.sh beside this file.
set -euo pipefail

# packaging/macos/bundle.sh, so the top of the tree is two levels up.
cd "$(dirname "$0")/../.."

binary="target/release/TinePlayer"
if [[ ! -x "$binary" ]]; then
    echo "No release build found. Run: cargo build --release" >&2
    exit 1
fi

# Taken from Cargo.toml so the bundle cannot claim a version the binary does
# not report.
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
app="dist/macos/TinePlayer.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/TinePlayer"

# iconutil wants a folder of PNGs at the sizes macOS draws, named the way it
# expects. Sizes larger than the source are left out rather than upscaled
# into blurry ones, so a bigger source simply fills more of them: at 1024 the
# full set is generated, including the 512@2x macOS uses for large previews.
#
# Its own artwork if there is any. macOS masks an application icon into the
# rounded square everything else in the Dock is, so what belongs here is a
# picture that fills the whole canvas - the opposite of what the application
# mark needs, which is drawn on a menu with the background showing through.
# Hence two files rather than one compromise.
iconset="$(mktemp -d)/TinePlayer.iconset"
mkdir -p "$iconset"
# A finished icon set, if there is one. Icon Composer and its like produce
# .icns directly, and what comes out of them carries more than a flat picture
# does - so it is used as it stands rather than rebuilt from an export.
if [[ -f data/branding/tineplayer.icns ]]; then
    cp data/branding/tineplayer.icns "$app/Contents/Resources/TinePlayer.icns"
    icns_ready=true
fi

icon_src="data/branding/tineplayer-macos.png"
[[ -f "$icon_src" ]] || icon_src="data/ui/tineplayer.png"
source_px="$(sips -g pixelWidth "$icon_src" | awk '/pixelWidth/{print $2}')"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$icon_src" \
        --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    if [[ $double -le $source_px ]]; then
        sips -z "$double" "$double" "$icon_src" \
            --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
    fi
done
if [[ "${icns_ready:-false}" != true ]]; then
    iconutil -c icns "$iconset" -o "$app/Contents/Resources/TinePlayer.icns"
fi
rm -rf "$(dirname "$iconset")"

# The identifier matches the GTK application id the running process registers,
# so macOS and the toolkit agree on what this application is.
cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>TinePlayer</string>
    <key>CFBundleDisplayName</key>
    <string>TinePlayer</string>
    <key>CFBundleIdentifier</key>
    <string>app.tineplayer.TinePlayer</string>
    <key>CFBundleExecutable</key>
    <string>TinePlayer</string>
    <key>CFBundleIconFile</key>
    <string>TinePlayer</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
</dict>
</plist>
PLIST

# Finder caches bundle metadata aggressively, and a bundle it has already seen
# can keep showing a generic icon otherwise.
touch "$app"

echo "Built $app"
echo "Open it with: open $app"
