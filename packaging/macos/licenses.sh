#!/usr/bin/env bash
# Collects the license paperwork for the libraries inside a macOS bundle.
#
# Homebrew has no equivalent of the license folder GStreamer's Windows build
# ships, so this works the problem backwards: for every library in the bundle,
# ask Homebrew which formula owns it, then take that formula's license text
# and recorded terms. What comes out describes the bundle rather than the
# machine it was built on.
#
# Called by package.sh. Takes the bundle and the Homebrew prefix.
set -euo pipefail

app="${1:?usage: licenses.sh <app> <brew-prefix>}"
prefix="${2:?usage: licenses.sh <app> <brew-prefix>}"
export PATH="$prefix/bin:$PATH"

resources="$app/Contents/Resources"
out="$resources/licenses"
mkdir -p "$out"

python3 - "$app" "$prefix" "$out" <<'PYTHON'
import json, os, subprocess, sys, shutil

app, prefix, out = sys.argv[1:4]

# Every library actually in the bundle, which is what has to be accounted for.
shipped = set()
for folder in ("Contents/Frameworks", "Contents/Resources/gstreamer-1.0",
               "Contents/Resources/gio-modules", "Contents/Resources/libexec"):
    path = os.path.join(app, folder)
    if os.path.isdir(path):
        shipped.update(os.listdir(path))

# Which formula owns which file. `brew list` for one formula at a time is slow
# enough to notice, so this asks once and builds the map.
formulas = subprocess.run(["brew", "list", "--formula"],
                          capture_output=True, text=True).stdout.split()
owner = {}
for formula in formulas:
    files = subprocess.run(["brew", "list", "--verbose", formula],
                           capture_output=True, text=True).stdout.splitlines()
    for path in files:
        owner.setdefault(os.path.basename(path), formula)

used = sorted({owner[name] for name in shipped if name in owner})
unaccounted = sorted(name for name in shipped if name not in owner)

# What each one is licensed under, as Homebrew records it.
terms = {}
if used:
    info = subprocess.run(["brew", "info", "--json=v2"] + used,
                          capture_output=True, text=True).stdout
    for entry in json.loads(info).get("formulae", []):
        terms[entry["name"]] = entry.get("license") or "see the project"

# The text itself, where the formula ships one.
taken = 0
for formula in used:
    cellar = subprocess.run(["brew", "--cellar", formula],
                            capture_output=True, text=True).stdout.strip()
    if not cellar or not os.path.isdir(cellar):
        continue
    versions = sorted(os.listdir(cellar))
    if not versions:
        continue
    root = os.path.join(cellar, versions[-1])
    for base, _, files in os.walk(root):
        # Only the top couple of levels: a source tree buried in a formula can
        # carry dozens of licenses belonging to its own dependencies.
        if base[len(root):].count(os.sep) > 1:
            continue
        for name in files:
            if name.split(".")[0].upper() in ("LICENSE", "COPYING", "COPYRIGHT", "NOTICE"):
                folder = os.path.join(out, formula)
                os.makedirs(folder, exist_ok=True)
                shutil.copy2(os.path.join(base, name), os.path.join(folder, name))
                taken += 1

with open(os.path.join(out, "BUNDLED-VERSIONS.txt"), "w") as f:
    f.write("Libraries in this bundle, the Homebrew formula each comes from,\n")
    f.write("and the terms Homebrew records for it.\n\n")
    for formula in used:
        version = subprocess.run(["brew", "list", "--versions", formula],
                                 capture_output=True, text=True).stdout.strip()
        f.write(f"{version or formula}\n    {terms.get(formula, 'unknown')}\n")
    if unaccounted:
        f.write("\nNot owned by any Homebrew formula (built into TinePlayer\n")
        f.write("itself, or part of macOS):\n")
        for name in unaccounted:
            f.write(f"    {name}\n")

print(f"  {len(used)} formulas, {taken} license files")
PYTHON

# The notice. Kept here rather than as a file so it cannot drift away from
# what the scripts actually do.
cat >"$out/README.md" <<'NOTICE'
# Licenses

TinePlayer is MIT licensed. Its own terms are in TinePlayer-MIT.txt, and the
Rust libraries compiled into it are listed with their licenses in
THIRD-PARTY.md, one folder up.

This bundle also carries libraries it did not write. BUNDLED-VERSIONS.txt
lists every one of them, the project it comes from and the terms it is under,
and the folders beside it hold those projects' license texts.

## GStreamer, GTK, GLib and the rest of the runtime

These are free software under the GNU Lesser General Public License. They are
in Contents/Frameworks, unmodified, exactly as Homebrew built them.

The LGPL asks that you be able to change them and still use this application
with your changed version. You can: they are ordinary shared libraries, loaded
by name, and replacing one with your own build of the same version is all that
is required. Nothing here is statically linked against them and nothing checks
their contents.

Their source, matching the versions in BUNDLED-VERSIONS.txt, is at:

  GStreamer   https://gitlab.freedesktop.org/gstreamer/gstreamer
  GTK         https://gitlab.gnome.org/GNOME/gtk
  GLib        https://gitlab.gnome.org/GNOME/glib
  FFmpeg      https://git.ffmpeg.org/ffmpeg.git
  libsoup     https://gitlab.gnome.org/GNOME/libsoup

Homebrew's own formulas, which record how each was built, are at
https://github.com/Homebrew/homebrew-core.

## gst-plugin-gtk4

The one library compiled into TinePlayer's executable rather than shipped
beside it, because it is not distributed as a plugin. It is under the Mozilla
Public License 2.0, which is file-scoped: it does not affect TinePlayer's own
terms, and its source remains available at
https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs.

## FFmpeg

FFmpeg is the one library here that does not come from Homebrew. Homebrew
builds it with the GPL parts enabled, linking x264 and x265, which would make
this whole bundle GPL. The copy here is the one GStreamer's own macOS build
uses, built without them and under the LGPL, taken from
https://gstreamer.freedesktop.org/data/pkg/osx/ for the same version of
GStreamer as everything else in the bundle.

## What is not here

No GPL-licensed plugin or library is included. AC-3, DTS, H.264, HEVC and AAC
are decoded by FFmpeg under the LGPL. x264 and x265, which Homebrew installs
alongside GStreamer, are GPL and are not in this bundle; they encode, which
nothing here does.

Patents are a separate matter from copyright, and vary by country. This bundle
decodes what the libraries above decode; whether a particular format needs a
patent licence where you are is not something this file can answer.
NOTICE
