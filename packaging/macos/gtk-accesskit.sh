#!/usr/bin/env bash
# Builds a GTK with the AccessKit backend, which Homebrew's does not have.
#
# Homebrew compiles gtk4 without it. Its own backend listing says so -
# "accesskit - Disabled during GTK build" - and the effect is that a screen
# reader sees the window and nothing inside it, however carefully every widget
# is named. VoiceOver support is not something TinePlayer can switch on from
# its own code; it is a property of the GTK it is linked against.
#
# So this builds one, into a prefix of its own, leaving Homebrew's alone. Used
# by developers on a Mac and by the release workflow, which is the point: a
# release built without it ships a player no screen reader can read, and
# nothing about that failure is visible from outside.
#
# Everything else - glib, pango, cairo, harfbuzz - still comes from Homebrew
# and is linked against, not rebuilt.
set -euo pipefail

prefix="${TINEPLAYER_GTK_PREFIX:-$HOME/gtk-a11y}"
work="${TINEPLAYER_GTK_WORK:-$HOME/src}"

export PATH="/opt/homebrew/bin:/usr/local/bin:$HOME/.cargo/bin:$PATH"

for tool in meson ninja cargo git pkg-config; do
    if ! command -v "$tool" >/dev/null; then
        echo "Missing $tool. Needs: brew install meson ninja, and a Rust toolchain." >&2
        exit 1
    fi
done

# --- Which GTK ----------------------------------------------------------
#
# The same version Homebrew has, rather than the newest.
#
# Ours links against Homebrew's glib, pango, cairo and harfbuzz, and the
# bundle ships those alongside it. Building a GTK from a different release
# than the stack around it is how a bundle comes to hold libraries that
# disagree about their own ABI - and the GSettings schemas the bundle takes
# from Homebrew are the gtk4 formula's, so they have to describe this GTK.
gtk_version="$(brew list --versions gtk4 2>/dev/null | awk '{print $2}')"
if [[ -z "$gtk_version" ]]; then
    echo "Homebrew has no gtk4 installed, so there is no version to match." >&2
    exit 1
fi
echo "Matching Homebrew's GTK $gtk_version"

mkdir -p "$work"

# --- GTK source ---------------------------------------------------------
gtk_src="$work/gtk"
if [[ -d "$gtk_src/.git" ]]; then
    git -C "$gtk_src" fetch --depth 1 origin "tag" "$gtk_version" -q 2>/dev/null || true
    git -C "$gtk_src" checkout -q "$gtk_version" 2>/dev/null || true
else
    git clone -q --depth 1 --branch "$gtk_version" \
        https://gitlab.gnome.org/GNOME/gtk.git "$gtk_src"
fi

# --- Which AccessKit ----------------------------------------------------
#
# Read out of GTK rather than pinned here. GTK asks for a particular C API
# version by pkg-config name - `dependency('accesskit-c-0.18')` at the time of
# writing - and the newest accesskit-c is not it: 0.22 installs a
# accesskit-c-0.22.pc that GTK will not look for, meson reports the dependency
# missing, and the backend is quietly left out of the build. Asking GTK which
# one it wants is what stops that recurring on the next GTK bump.
api="$(grep -o "accesskit-c-[0-9]\+\.[0-9]\+" "$gtk_src/meson.build" | head -1)"
if [[ -z "$api" ]]; then
    echo "Could not find which accesskit-c API GTK $gtk_version wants." >&2
    echo "Look for dependency('accesskit-c-...') in $gtk_src/meson.build." >&2
    exit 1
fi
wanted="${api#accesskit-c-}"
echo "GTK $gtk_version wants accesskit-c $wanted"

# The tag matching that API. accesskit-c tags are plain versions, and the API
# in the pkg-config name is the major.minor of one of them.
ak_src="$work/accesskit-c"
if [[ -d "$ak_src/.git" ]]; then
    git -C "$ak_src" fetch --tags -q
else
    git clone -q https://github.com/AccessKit/accesskit-c.git "$ak_src"
fi
tag="$(git -C "$ak_src" tag | grep "^${wanted}\." | sort -V | tail -1)"
if [[ -z "$tag" ]]; then
    echo "No accesskit-c release matches API $wanted." >&2
    exit 1
fi
echo "Building accesskit-c $tag"
git -C "$ak_src" checkout -q "$tag"

# --- Build ---------------------------------------------------------------
rm -rf "$ak_src/build"
meson setup "$ak_src/build" --prefix="$prefix" --buildtype=release >/dev/null
meson compile -C "$ak_src/build" >/dev/null
meson install -C "$ak_src/build" >/dev/null

# The paperwork, beside the library it belongs to. `licenses.sh` collects
# license text by asking Homebrew which formula owns each bundled file, and
# nothing here is a Homebrew formula - so without this the bundle ships a
# library with no terms anywhere in it, which is exactly what packaging is
# supposed to prevent.
licenses="$prefix/share/licenses/accesskit-c"
mkdir -p "$licenses"
for name in LICENSE-APACHE LICENSE-MIT COPYING.LIB LICENSE.chromium AUTHORS; do
    [[ -f "$ak_src/$name" ]] && cp "$ak_src/$name" "$licenses/"
done
echo "accesskit-c $tag" > "$licenses/VERSION"

rm -rf "$gtk_src/_build"
PKG_CONFIG_PATH="$prefix/lib/pkgconfig:${PKG_CONFIG_PATH:-}" \
meson setup "$gtk_src/_build" \
    --prefix="$prefix" --buildtype=release \
    -Daccesskit=enabled \
    -Dbuild-examples=false -Dbuild-tests=false -Dbuild-demos=false \
    -Dbuild-testsuite=false -Dintrospection=disabled -Dman-pages=false \
    -Ddocumentation=false -Dmedia-gstreamer=disabled -Dvulkan=disabled \
    -Dx11-backend=false -Dmacos-backend=true >/dev/null

# Asserted rather than assumed. meson reports a missing optional dependency as
# a line in a summary nobody reads, and the build then succeeds without the
# backend - producing exactly the GTK this script exists to replace.
if ! grep -q "AccessKit support: true" "$gtk_src/_build/meson-logs/meson-log.txt"; then
    echo "GTK configured WITHOUT AccessKit. Not building it." >&2
    grep -i accesskit "$gtk_src/_build/meson-logs/meson-log.txt" | tail -5 >&2
    exit 1
fi

meson compile -C "$gtk_src/_build" >/dev/null
meson install -C "$gtk_src/_build" >/dev/null

gtk_licenses="$prefix/share/licenses/gtk4-accesskit"
mkdir -p "$gtk_licenses"
for name in COPYING; do
    [[ -f "$gtk_src/$name" ]] && cp "$gtk_src/$name" "$gtk_licenses/"
done
echo "gtk $gtk_version, built with -Daccesskit=enabled" > "$gtk_licenses/VERSION"

# --- Say what came out ---------------------------------------------------
line="$(strings "$prefix/lib/libgtk-4.dylib" | grep -m1 "accesskit -" || true)"
echo
echo "Installed to $prefix"
echo "  GTK $gtk_version, accesskit-c $tag"
echo "  backend: ${line# *}"
echo
echo "Build against it with:"
echo "  export TINEPLAYER_GTK_PREFIX=$prefix"
echo "  export PKG_CONFIG_PATH=$prefix/lib/pkgconfig:\$PKG_CONFIG_PATH"
echo "  export RUSTFLAGS=\"-L native=$prefix/lib\""
