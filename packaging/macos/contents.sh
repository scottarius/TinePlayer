#!/usr/bin/env bash
# Fills the app bundle with everything it needs to run on a Mac that has no
# Homebrew: the GStreamer plugins, the libraries they and the binary link, the
# GIO modules, the data GTK reads, and the license paperwork for all of it.
#
# Run by package.sh, which creates the bundle first.
#
# Three things this has to get right, all of which cost time to learn:
#   - the plugin search path must *replace* the built-in one, or Homebrew's
#     plugins load too and bring a second glib into the process;
#   - libraries opened by name at runtime are invisible to dependency walking,
#     which is what OPENED_BY_NAME is for;
#   - plugins are chosen rather than swept up, because some are GPL and some
#     drag in entire runtimes.
set -euo pipefail

cd "$(dirname "$0")/../.."

app="dist/macos/TinePlayer.app"
if [[ ! -d "$app" ]]; then
    echo "No bundle found. Run: ./packaging/macos/bundle.sh" >&2
    exit 1
fi

export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
prefix="$(brew --prefix)"

frameworks="$app/Contents/Frameworks"
resources="$app/Contents/Resources"
plugins="$resources/gstreamer-1.0"
mkdir -p "$frameworks" "$plugins" "$resources/share" "$resources/licenses"

# --- What the player needs ---------------------------------------------
#
# The same set the Windows package takes, in macOS names, chosen for the same
# reasons: every one is LGPL, BSD, MIT or ISC. Nothing GPL is here. x264 is
# the one obviously GPL plugin Homebrew installs, and it encodes anyway, which
# nothing here does.
#
# AC-3, DTS, H.264, HEVC and AAC are decoded by FFmpeg, which does not come
# from Homebrew - see the LGPL FFmpeg step further down.
PLUGINS=(
    # The pipeline itself.
    coreelements playback typefindfunctions autodetect

    # Audio, both outputs. osxaudio is also where the device list comes from.
    audioconvert audioresample audiofx volume osxaudio wavparse

    # Containers, matching every extension the browser offers.
    matroska isomp4 avi ogg mpegtsdemux mpegpsdemux asf flv id3demux apetag mxf dv

    # Streaming, for a URL naming a playlist.
    hls dash

    # Parsers, between demuxer and decoder.
    audioparsers videoparsersbad

    # Decoders free of licence and patent trouble. FFmpeg is not here: see
    # the LGPL FFmpeg step below for why it comes from somewhere else.
    vpx dav1d opus vorbis flac alaw mulaw

    # Video output. applemedia is VideoToolbox, so decoding runs on the GPU.
    videoconvertscale videofilter opengl applemedia

    # Subtitles, and the text drawing they need.
    subparse assrender pango closedcaption dvdsub

    # Playing from a URL.
    soup tcp
)

# Opened by name at runtime, so nothing links them and walking finds nothing.
# This is the gap that left the earlier attempt unable to open http:// at all.
OPENED_BY_NAME=(
    libsoup-3.0.0.dylib
    libpsl.5.dylib
    libnghttp2.14.dylib
    libbrotlidec.1.dylib
)

echo "Copying GStreamer plugins..."
missing=()
for name in "${PLUGINS[@]}"; do
    file="$prefix/lib/gstreamer-1.0/libgst$name.dylib"
    if [[ -f "$file" ]]; then
        cp "$file" "$plugins/"
    else
        missing+=("libgst$name")
    fi
done
chmod u+w "$plugins"/*.dylib
[[ ${#missing[@]} -gt 0 ]] && echo "  not in this Homebrew, skipped: ${missing[*]}"

# Opened-by-name libraries go in first, so the walk below picks up whatever
# they themselves need.
for name in "${OPENED_BY_NAME[@]}"; do
    if [[ -f "$prefix/lib/$name" ]]; then
        cp "$prefix/lib/$name" "$frameworks/"
        chmod u+w "$frameworks/$name"
    else
        echo "  opened-by-name library absent: $name"
    fi
done

# GStreamer scans plugins in a helper process rather than its own, and without
# this it says so at every startup.
scanner="$(find "$prefix" -name gst-plugin-scanner -type f 2>/dev/null | head -1)"
if [[ -n "$scanner" ]]; then
    mkdir -p "$resources/libexec"
    cp "$scanner" "$resources/libexec/"
    chmod u+w "$resources/libexec/gst-plugin-scanner"
fi

# glib loads these at runtime from wherever it was built, which is how a
# second copy of libgio was getting into the process alongside the bundled
# one. Carried rather than merely blocked, because this is where TLS comes
# from: without it an https:// address cannot be opened at all.
if [[ -d "$prefix/lib/gio/modules" ]]; then
    echo "Copying GIO modules..."
    mkdir -p "$resources/gio-modules"
    for module in "$prefix/lib/gio/modules/"*.so "$prefix/lib/gio/modules/"*.dylib; do
        [[ -f "$module" ]] && cp "$module" "$resources/gio-modules/"
    done
    chmod u+w "$resources/gio-modules/"* 2>/dev/null || true
fi

# --- FFmpeg, under the LGPL ---------------------------------------------
#
# Not Homebrew's. Homebrew builds FFmpeg with --enable-gpl, linking x264 and
# x265, so its libavcodec is GPL-3.0 - and bundling it would make this whole
# application GPL, ruling out the App Store and putting an obligation to ship
# source on every release.
#
# GStreamer's own macOS build has no such problem: cerbero builds FFmpeg
# without the GPL parts and keeps GPL codecs in separate packages nobody has
# to take. Its libav plugin is built against exactly the GStreamer version
# Homebrew has here, so it drops straight in.
#
# The download is large and rarely changes, so it is kept under target/.
gst_version="$(brew list --versions gstreamer | awk '{print $2}')"
cache="target/packaging-cache"
pkg="$cache/gstreamer-$gst_version-universal.pkg"
expanded="$cache/gstreamer-$gst_version"

mkdir -p "$cache"
if [[ ! -f "$pkg" ]]; then
    echo "Fetching GStreamer $gst_version for macOS (LGPL FFmpeg)..."
    curl -fL --progress-bar -o "$pkg"         "https://gstreamer.freedesktop.org/data/pkg/osx/$gst_version/gstreamer-1.0-$gst_version-universal.pkg" ||
        {
            echo "Could not fetch GStreamer $gst_version. Without it there is no" >&2
            echo "LGPL FFmpeg, and AC-3 and DTS will not play." >&2
            exit 1
        }
fi
if [[ ! -d "$expanded" ]]; then
    pkgutil --expand-full "$pkg" "$expanded" >/dev/null
fi

libav_payload="$expanded/gstreamer-1.0-libav-$gst_version-universal.pkg/Payload"
if [[ ! -d "$libav_payload" ]]; then
    echo "No libav payload in the downloaded package." >&2
    exit 1
fi

echo "Copying LGPL FFmpeg..."
cp "$libav_payload/lib/gstreamer-1.0/libgstlibav.dylib" "$plugins/"
for lib in "$libav_payload/lib/"libav*.dylib "$libav_payload/lib/"libsw*.dylib; do
    [[ -f "$lib" ]] || continue
    cp "$lib" "$frameworks/"
done
chmod u+w "$plugins/libgstlibav.dylib" "$frameworks/"libav*.dylib "$frameworks/"libsw*.dylib 2>/dev/null || true

# What those in turn need, which is not necessarily in the same sub-package
# and is not in Homebrew either: FFmpeg wants libbz2, and macOS keeps its own
# in /usr/lib where a bundle cannot take it from. Resolved against the whole
# downloaded package rather than guessing.
for name in $(otool -L "$frameworks"/libav*.dylib "$frameworks"/libsw*.dylib 2>/dev/null |
    awk '/^	@rpath\//{sub("@rpath/","",$1); print $1}' | sort -u); do
    case "$name" in libav* | libsw*) continue ;; esac
    [[ -f "$frameworks/$name" ]] && continue
    found="$(find "$expanded" -name "$name" -type f 2>/dev/null | head -1)"
    if [[ -n "$found" ]]; then
        cp "$found" "$frameworks/"
        chmod u+w "$frameworks/$name"
    else
        echo "  FFmpeg wants $name and it is not in the package" >&2
    fi
done

# Nothing GPL should have arrived with it.
if ls "$frameworks"/libx264* "$frameworks"/libx265* >/dev/null 2>&1; then
    echo "GPL encoders ended up in the bundle. Stopping rather than shipping that." >&2
    exit 1
fi

# --- Every library any of that links ------------------------------------
echo "Copying libraries..."
python3 - "$prefix" "$frameworks" "$app/Contents/MacOS/TinePlayer" "$plugins" <<'PYTHON'
import os, subprocess, shutil, sys

prefix, frameworks, binary, plugins = sys.argv[1:5]


def resolve(dep):
    """Where a dependency actually lives, or None to leave it alone.

    Homebrew names its libraries by @rpath rather than by absolute path, so
    following only the absolute ones walks a fraction of the graph: libwebp
    asks for @rpath/libsharpyuv.0.dylib, which never appears as a path at all.
    System libraries under /usr/lib and /System are deliberately not resolved,
    since every Mac has them and copying them in would be wrong.
    """
    if dep.startswith(prefix):
        return dep
    if dep.startswith("@rpath/"):
        candidate = os.path.join(prefix, "lib", dep[len("@rpath/"):])
        return candidate if os.path.exists(candidate) else None
    return None


def linked(path):
    out = subprocess.run(["otool", "-L", path], capture_output=True, text=True).stdout
    found = []
    for line in out.splitlines()[1:]:
        dep = line.split()[0]
        # A library's own install id is printed first, among its dependencies
        # and looking exactly like one. Following it copies every plugin into
        # Frameworks as well as its own directory, which is how the bundle
        # came to hold two of each.
        if os.path.basename(dep) == os.path.basename(path):
            continue
        target = resolve(dep)
        if target:
            found.append(target)
    return found


roots = [binary] + [os.path.join(plugins, f) for f in os.listdir(plugins)]
for extra in ("gio-modules", "libexec"):
    folder = os.path.join(os.path.dirname(plugins), extra)
    if os.path.isdir(folder):
        roots += [os.path.join(folder, f) for f in os.listdir(folder)
                  if not f.endswith(".cache")]
# The opened-by-name libraries were copied in before this ran.
roots += [os.path.join(frameworks, f) for f in os.listdir(frameworks)]

seen, queue = set(), [d for r in roots for d in linked(r)]
while queue:
    lib = queue.pop()
    # Keyed on the file name rather than the path it was reached by: Homebrew
    # refers to the same library through both its Cellar path and its opt
    # symlink, and copying it twice fails the second time, since what landed
    # the first time is read-only.
    name = os.path.basename(lib)
    real = os.path.realpath(lib)
    if name in seen or not os.path.exists(real):
        continue
    seen.add(name)
    copied = os.path.join(frameworks, name)
    if not os.path.exists(copied):
        shutil.copy2(real, copied)
    # Homebrew ships these read-only, and install_name_tool has to rewrite
    # every one of them in a moment.
    os.chmod(copied, 0o644)
    queue.extend(linked(real))
print(f"  {len(os.listdir(frameworks))} libraries")
PYTHON

# --- Point everything at the bundle -------------------------------------
echo "Repointing libraries..."
retarget() {
    local file="$1"
    otool -L "$file" | awk -v p="$prefix" 'NR>1 && index($1, p) == 1 {print $1}' |
        while read -r dep; do
            install_name_tool -change "$dep" "@rpath/$(basename "$dep")" "$file" 2>/dev/null || true
        done
}
for lib in "$frameworks"/*.dylib; do retarget "$lib"; done
for plugin in "$plugins"/*.dylib; do retarget "$plugin"; done
for module in "$resources/gio-modules/"*; do
    [[ -f "$module" ]] && retarget "$module"
done
retarget "$app/Contents/MacOS/TinePlayer"

# Where @rpath resolves to, from each of the places things load from.
install_name_tool -add_rpath "@executable_path/../Frameworks" \
    "$app/Contents/MacOS/TinePlayer" 2>/dev/null || true
for lib in "$frameworks"/*.dylib; do
    install_name_tool -add_rpath "@loader_path" "$lib" 2>/dev/null || true
done
for plugin in "$plugins"/*.dylib; do
    install_name_tool -add_rpath "@loader_path/../../Frameworks" "$plugin" 2>/dev/null || true
done
for module in "$resources/gio-modules/"*; do
    [[ -f "$module" ]] || continue
    install_name_tool -add_rpath "@loader_path/../../Frameworks" "$module" 2>/dev/null || true
done
if [[ -f "$resources/libexec/gst-plugin-scanner" ]]; then
    retarget "$resources/libexec/gst-plugin-scanner"
    install_name_tool -add_rpath "@loader_path/../../Frameworks" \
        "$resources/libexec/gst-plugin-scanner" 2>/dev/null || true
fi

# --- What GTK reads at runtime ------------------------------------------
echo "Copying GTK runtime data..."
mkdir -p "$resources/share/glib-2.0/schemas"
cp "$prefix/share/glib-2.0/schemas/gschemas.compiled" \
    "$resources/share/glib-2.0/schemas/" 2>/dev/null || true

# GTK on macOS draws its icons from a theme on disk rather than from anything
# built in, so without this the buttons are missing-image boxes.
for theme in Adwaita hicolor; do
    if [[ -d "$prefix/share/icons/$theme" ]]; then
        mkdir -p "$resources/share/icons"
        cp -R "$prefix/share/icons/$theme" "$resources/share/icons/"
    fi
done

# --- The paperwork ------------------------------------------------------
echo "Collecting licenses..."
cp LICENSE "$resources/licenses/TinePlayer-MIT.txt"
[[ -f THIRD-PARTY.md ]] && cp THIRD-PARTY.md "$resources/"
./packaging/macos/licenses.sh "$app" "$prefix"

# --- Sign ----------------------------------------------------------------
# Ad-hoc, which is not notarization: it makes the bundle run on the machine
# that built it, and on any Mac where the user chooses to allow it, but a copy
# downloaded from the internet is still quarantined until someone opens it
# from the right-click menu. Notarization needs a paid Apple developer
# account, and is a decision for release rather than a step here.
#
# Every library on its own, innermost first, and the bundle last. --deep does
# not do this properly: it leaves nested libraries with the signatures they
# had before install_name_tool rewrote them, and macOS then kills the process
# on launch with no message beyond SIGKILL.
echo "Signing (ad-hoc)..."
find "$app" \( -name "*.dylib" -o -name "*.so" \) -print0 |
    xargs -0 -n1 codesign --force --timestamp=none --sign - 2>/dev/null || true
[[ -f "$resources/libexec/gst-plugin-scanner" ]] &&
    codesign --force --sign - "$resources/libexec/gst-plugin-scanner" 2>/dev/null || true
codesign --force --sign - "$app/Contents/MacOS/TinePlayer" 2>/dev/null || true
codesign --force --sign - "$app" 2>/dev/null ||
    echo "  codesign failed, which only matters once this is distributed"

if ! codesign --verify --deep "$app" 2>/dev/null; then
    echo "  signature does not verify; the bundle will be killed on launch" >&2
fi

size="$(du -sh "$app" | cut -f1)"
echo
echo "Bundled $app ($size)"
echo
echo "Test it on a Mac without Homebrew, which is the only test that proves"
echo "it is self-contained."
