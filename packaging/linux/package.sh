#!/usr/bin/env bash
# Builds the Linux package: a .deb for Debian, Ubuntu and Raspberry Pi OS.
#
# Produces dist/linux/tineplayer_<version>_linux_<arch>.deb, installed with
#
#     sudo apt install ./tineplayer_1.0.0_linux_arm64.deb
#
# The ./ matters. It is what makes apt treat the file as a package to install
# *and* resolve dependencies for; plain `dpkg -i` installs it and then leaves
# the system needing `apt --fix-broken install` when GStreamer is not already
# there.
#
# Nothing is bundled. GTK, GStreamer and the codecs are declared as
# dependencies and come from the distribution, which is the whole reason this
# is a few megabytes rather than a few hundred, and the reason it carries no
# license obligations for other people's libraries: depending on software is
# not distributing it.
#
# A package is architecture specific, since it contains a compiled binary, so
# amd64 and arm64 are separate builds of this same script. Run it on the
# architecture you are building for - natively on a runner, or under emulation
# in the container in ../Dockerfile.
set -euo pipefail

cd "$(dirname "$0")/../.."

package="tineplayer"
version="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
arch="$(dpkg --print-architecture)"
maintainer="Scott Bounds <scott.bounds@gmail.com>"
homepage="https://github.com/scottarius/TinePlayer"
app_id="app.tineplayer.TinePlayer"

# "linux" in the name, which Debian's own convention leaves out because on a
# Debian system there is nothing else it could be. On a releases page beside a
# .dmg and two Windows downloads, it is the thing a reader is looking for.
# Nothing reads the file name: apt and dpkg both go by what is inside.
stage="dist/linux/${package}_${version}_linux_${arch}"
deb="dist/linux/${package}_${version}_linux_${arch}.deb"

for tool in dpkg-deb dpkg-shlibdeps; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "$tool is needed to build the Linux package." >&2
        echo "  apt install dpkg-dev" >&2
        exit 1
    }
done

echo "TinePlayer: Debian package ($arch)"

if [[ "${TINE_SKIP_BUILD:-}" != "1" ]]; then
    echo "Building..."
    cargo build --release
fi

# Where cargo actually put it, rather than assuming ./target. CARGO_TARGET_DIR
# moves it, which is what the build container sets so that a bind-mounted
# source tree is not written to - and assuming ./target there packaged
# whatever stale binary happened to be lying in the source tree instead of the
# one just built. On a foreign architecture that failed loudly at `strip`; on
# a matching one it would have shipped silently.
target_dir="${CARGO_TARGET_DIR:-target}"
built="$target_dir/release/TinePlayer"
[[ -x "$built" ]] || {
    echo "No release build found at $built." >&2
    exit 1
}

# And that it is for this machine, since being handed the wrong architecture
# is exactly the failure this is guarding against and it is worth saying so
# rather than letting a later step produce a puzzling error.
if command -v file >/dev/null 2>&1; then
    expected="$(case "$arch" in
        amd64) echo "x86-64" ;;
        arm64) echo "aarch64" ;;
        armhf) echo "ARM" ;;
        *) echo "" ;;
    esac)"
    if [[ -n "$expected" ]] && ! file -b "$built" | grep -q "$expected"; then
        echo "The binary at $built is not $arch:" >&2
        echo "  $(file -b "$built")" >&2
        exit 1
    fi
fi

rm -rf "$stage"
mkdir -p "$stage/DEBIAN"

# --- What goes where ----------------------------------------------------
# Lowercase on Linux, where that is the convention and what somebody would
# type, while Windows and macOS keep the capitalized name. The application id,
# and so the icon and desktop file names, stay the same everywhere.
install -Dm755 "$built" "$stage/usr/bin/$package"
# Debug symbols are most of the binary and no use in a package: anyone
# debugging builds from source. Kept out of cargo's own profile so that a
# developer's release build stays debuggable.
strip --strip-unneeded "$stage/usr/bin/$package"
install -Dm644 "data/branding/$app_id.svg" \
    "$stage/usr/share/icons/hicolor/scalable/apps/$app_id.svg"

# The fonts go where fontconfig already looks, rather than beside the
# executable: on Linux there is a system font path and using it means the
# application needs no configuration of its own to find them. This is the one
# platform where use_bundled_fonts finds nothing and does nothing, which is
# the intended outcome.
#
# They are here at all because no distribution reliably has all of these
# scripts - Raspberry Pi OS has no Korean, Chinese or Telugu font at all - and
# a hundred megabytes of Noto as a dependency is out of proportion to drawing
# a menu of language names.
fonts=$(ls data/fonts/*.ttf 2>/dev/null | wc -l)
[[ "$fonts" -gt 0 ]] || {
    echo "No fonts in data/fonts. Run packaging/fonts/build-fonts.py first." >&2
    exit 1
}
echo "Fonts: $fonts"
for font in data/fonts/*.ttf; do
    install -Dm644 "$font" "$stage/usr/share/fonts/truetype/$package/$(basename "$font")"
done

# A command on PATH is expected to answer `man`, and this one has enough
# options to be worth the page.
install -Dm644 packaging/linux/tineplayer.1 "$stage/usr/share/man/man1/$package.1"
gzip -9n "$stage/usr/share/man/man1/$package.1"

# Exec and StartupWMClass are rewritten for where this actually installs to.
# The template names the binary as a source build has it; here it is on PATH
# under its packaged name, and X11 takes the window class from that same name.
mkdir -p "$stage/usr/share/applications"
sed -e "s|^Exec=.*|Exec=$package %f|" \
    -e "s|^StartupWMClass=.*|StartupWMClass=$package|" \
    "data/templates/$app_id.desktop" \
    >"$stage/usr/share/applications/$app_id.desktop"
chmod 644 "$stage/usr/share/applications/$app_id.desktop"

# --- The paperwork Debian expects ---------------------------------------
docs="$stage/usr/share/doc/$package"
mkdir -p "$docs"
install -Dm644 THIRD-PARTY.md "$docs/THIRD-PARTY.md"
# The fonts are under the SIL Open Font License, which requires its text
# to travel with them.
install -Dm644 data/fonts/OFL.txt "$docs/NotoFonts-OFL.txt"

# Machine-readable copyright, which is the format Debian tooling and license
# scanners read. TinePlayer's own terms; the libraries it depends on carry
# their own, in their own packages, which is what depending rather than
# bundling means.
cat >"$docs/copyright" <<COPYRIGHT
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: TinePlayer
Upstream-Contact: $maintainer
Source: $homepage

Files: *
Copyright: 2026 Scott Bounds
License: MIT

Comment:
 TinePlayer is written in Rust, so the libraries it is built from are compiled
 into the executable rather than loaded from the system. Every one of them,
 with its version and its license, is listed in THIRD-PARTY.md beside this
 file. They are permissively licensed: MIT, Apache-2.0, BSD, ISC, or Zlib.
 .
 GTK, GStreamer and FFmpeg are the exception. Those are loaded at runtime and
 are not part of this package at all: they are declared as dependencies and
 installed by apt from your distribution, under its terms.

Files: usr/share/fonts/*
Copyright: The Noto Project Authors
License: OFL-1.1
Comment:
 Cut down from Noto Sans to the characters TinePlayer draws, and renamed,
 which the licence requires of a modified copy. The full text is in
 NotoFonts-OFL.txt beside this file.

License: MIT
$(sed 's/^$/./; s/^/ /' LICENSE)

License: OFL-1.1
$(sed 's/^$/./; s/^/ /' data/fonts/OFL.txt)
COPYRIGHT
chmod 644 "$docs/copyright"

# lintian reports the Rust port of libyaml, reached through serde_yaml, as an
# embedded library. It is right that the code is in there, and wrong that it
# is a problem: a Rust binary statically links everything it uses, and the
# alternative lintian has in mind - a shared system copy that security updates
# reach - does not exist for a crate. Overridden so that the next real finding
# is not lost in a warning that will never change, with the reason recorded
# where anyone auditing the package will read it.
install -d "$stage/usr/share/lintian/overrides"
cat >"$stage/usr/share/lintian/overrides/$package" <<OVERRIDE
# Statically linked Rust crate (unsafe-libyaml, via serde_yaml), which is how
# every Rust binary is built. Licensed MIT and listed in THIRD-PARTY.md.
$package: embedded-library libyaml [usr/bin/$package]
OVERRIDE
chmod 644 "$stage/usr/share/lintian/overrides/$package"

# Policy asks for a changelog, compressed, and complains in ways that look
# like a broken package when it is missing.
# changelog.gz rather than changelog.Debian.gz: the version has no Debian
# revision on it, which makes this a native package, and a native package
# keeps its changelog under the plain name.
cat >"$docs/changelog" <<CHANGELOG
$package ($version) stable; urgency=medium

  * TinePlayer $version.
  * Release notes: $homepage/releases

 -- $maintainer  $(date -R)
CHANGELOG
gzip -9n "$docs/changelog"
chmod 644 "$docs/changelog.gz"

# --- What it needs to run -----------------------------------------------
# The libraries the binary links are worked out from the binary itself rather
# than written down here, because a hand-kept list is a list that drifts:
# dpkg-shlibdeps reads the ELF and asks dpkg which package owns each library
# and which version first provided the symbols used.
echo "Working out dependencies..."
# dpkg-shlibdeps insists on being run from a source package, so it wants a
# debian/control beside it. That scratch copy goes in a temporary directory
# rather than inside the staging tree, because the tree already has a DEBIAN
# directory and on a case-insensitive filesystem - a Windows or macOS bind
# mount, say - `debian` and `DEBIAN` are the same directory. Cleaning up the
# scratch then deletes the package metadata, and the failure lands several
# steps later looking like nothing to do with it.
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/debian"
printf 'Source: %s\n\nPackage: %s\nArchitecture: %s\n' \
    "$package" "$package" "$arch" >"$scratch/debian/control"
binary="$PWD/$stage/usr/bin/$package"
linked="$(cd "$scratch" && dpkg-shlibdeps -O --ignore-missing-info "$binary" 2>/dev/null |
    sed 's/^shlibs:Depends=//')"
[[ -n "$linked" ]] || {
    echo "dpkg-shlibdeps found nothing, which cannot be right." >&2
    exit 1
}

# GStreamer plugins are opened by name at runtime, so nothing links them and
# the step above cannot see them. They are the difference between a player
# that starts and a player that plays, so they are dependencies rather than
# recommendations.
#
#   base/good  containers, parsers, and the ordinary codecs
#   bad        HLS, DASH, subtitle rendering, hardware decoding
#   libav      AC-3, DTS and everything else FFmpeg decodes. Debian builds it
#              against a GPL FFmpeg, which is fine here in a way it was not
#              for a bundle: we depend on it, apt installs it, and nobody is
#              redistributing it as part of TinePlayer
#   gl         the OpenGL path the video sink uses where it can
plugins="gstreamer1.0-plugins-base, gstreamer1.0-plugins-good"
plugins="$plugins, gstreamer1.0-plugins-bad, gstreamer1.0-libav, gstreamer1.0-gl"
# One of the two, not both: a desktop has PulseAudio or PipeWire's stand-in
# for it, and a bare system has ALSA. Listing an alternative lets either
# satisfy it rather than dragging a sound server onto a machine that has one.
plugins="$plugins, gstreamer1.0-pulseaudio | gstreamer1.0-alsa"

# What it takes up once unpacked, which is what apt reports before installing.
# The DEBIAN directory is the packaging's own metadata and is not installed,
# so it does not count.
installed_kb="$(du -sk --exclude=DEBIAN "$stage" | cut -f1)"

cat >"$stage/DEBIAN/control" <<CONTROL
Package: $package
Version: $version
Section: video
Priority: optional
Architecture: $arch
Maintainer: $maintainer
Homepage: $homepage
Installed-Size: $installed_kb
Depends: $linked, $plugins
Description: Watch one film together, each with your own soundtrack
 TinePlayer plays one video with two soundtracks at once, sending each to a
 different output device. Two people watch the same screen together: one hears
 the film in English through the television, the other in Spanish through
 headphones, and both stay in sync because it is all one pipeline decoding one
 file.
 .
 The same split carries audio description, so someone who needs the narrated
 version hears it on headphones while everyone else hears the ordinary
 soundtrack.
 .
 The interface is sized to be read from across a room and can be driven by
 keyboard or gamepad alone. It remembers where you stopped, handles subtitles
 from embedded tracks or external files, and can register itself with Kodi as
 an external player.
CONTROL

# --- Telling the desktop it is there ------------------------------------
# The menu entry needs no help: desktop-file-utils watches
# /usr/share/applications and rebuilds its database itself whenever anything
# lands there. Icons have no such trigger, so the cache is rebuilt here or the
# entry can appear with a blank icon until something else happens to refresh
# it. Guarded because update-icon-caches belongs to a package that a headless
# system may not have, and a missing icon cache is not worth a failed install.
for script in postinst postrm; do
    cat >"$stage/DEBIAN/$script" <<'HOOK'
#!/bin/sh
set -e
if command -v update-icon-caches >/dev/null 2>&1; then
    update-icon-caches /usr/share/icons/hicolor
fi
# Debian's fontconfig ships a trigger on /usr/share/fonts and will usually do
# this itself; this is for the systems where it does not, and costs a second.
if command -v fc-cache >/dev/null 2>&1; then
    fc-cache -f >/dev/null 2>&1 || true
fi
HOOK
    chmod 755 "$stage/DEBIAN/$script"
done

# --- Build it -----------------------------------------------------------
echo "Packaging..."
rm -f "$deb"
# Directory permissions, flattened before dpkg-deb sees them, which refuses
# anything outside 0755-0775. --root-owner-group settles ownership but not the
# mode bits, and a staging tree inherits whatever the filesystem under it does.
find "$stage" -type d -exec chmod 755 {} +

# Some filesystems ignore that. An SMB mount with setgid directories keeps
# 2755 however often it is asked not to, and dpkg-deb then fails with a
# message that says nothing about where the problem is.
mode="$(stat -c %a "$stage/DEBIAN")"
if [[ "$mode" != "755" && "$mode" != "775" ]]; then
    echo "The staging tree is on a filesystem that will not take ordinary" >&2
    echo "directory permissions: $stage/DEBIAN is $mode and cannot be changed." >&2
    echo "dpkg-deb accepts only 0755 to 0775, so build somewhere local - a" >&2
    echo "network share is no good for this step." >&2
    exit 1
fi
# --root-owner-group so the package does not carry whatever user id happened
# to run the build, which is what makes a package built in a container install
# with sane ownership.
dpkg-deb --build --root-owner-group "$stage" "$deb" >/dev/null

# --- Check it -----------------------------------------------------------
# Best effort: lintian is not installed everywhere, and its complaints are
# worth reading rather than worth failing a build over.
if command -v lintian >/dev/null 2>&1; then
    echo "Checking with lintian..."
    lintian --no-tag-display-limit "$deb" 2>&1 | sed 's/^/  /' || true
fi

size="$(du -h "$deb" | cut -f1)"
echo
echo "Built $deb ($size)"
echo
echo "Install it with:  sudo apt install ./$deb"
