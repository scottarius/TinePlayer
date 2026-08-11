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

# Anything shipped from outside Homebrew, which owns its own paperwork.
#
# A GTK built with the AccessKit backend does not come from a formula, and
# neither does accesskit-c beside it, so the ownership map above cannot see
# either. `gtk-accesskit.sh` puts their license texts under share/licenses in
# its prefix precisely so this can find them - without it the bundle ships a
# library whose terms appear nowhere, and BUNDLED-VERSIONS.txt files it under
# "built into TinePlayer itself", which is not true of somebody else's library.
outside = []
gtk_prefix = os.environ.get("TINEPLAYER_GTK_PREFIX", "")
extra = os.path.join(gtk_prefix, "share", "licenses") if gtk_prefix else ""
if extra and os.path.isdir(extra):
    for project in sorted(os.listdir(extra)):
        source = os.path.join(extra, project)
        if not os.path.isdir(source):
            continue
        folder = os.path.join(out, project)
        os.makedirs(folder, exist_ok=True)
        described = project
        for name in sorted(os.listdir(source)):
            if name == "VERSION":
                with open(os.path.join(source, name)) as v:
                    described = v.read().strip() or project
                continue
            shutil.copy2(os.path.join(source, name), os.path.join(folder, name))
            taken += 1
        outside.append((project, described))

# Which of the unaccounted files those cover, so the report does not claim a
# library is unaccounted for when its license is sitting beside it. Matched on
# the project name appearing in the file name, which is how these are named:
# libaccesskit-c-0.18.0.dylib against accesskit-c.
covered = {name for name in unaccounted
           for project, _ in outside if project in name}
unaccounted = [name for name in unaccounted if name not in covered]

# Libraries deliberately taken from somewhere that is not Homebrew and not a
# prefix we built. Homebrew cannot name them and neither can share/licenses,
# so they are named here or they are named nowhere.
#
# FFmpeg is here on purpose: Homebrew builds it with x264 and x265, which is
# GPL and would take the whole bundle with it, so the copy shipped is the one
# from GStreamer's own macOS build, LGPL and without them. bzip2 comes along
# because FFmpeg wants it and the system's own is not usable from a bundle.
# Both were previously filed under "built into TinePlayer itself, or part of
# macOS", which is untrue of each.
ELSEWHERE = (
    ("libav", "FFmpeg, from GStreamer's macOS build",
     "LGPL-2.1-or-later", "https://ffmpeg.org"),
    ("libsw", "FFmpeg, from GStreamer's macOS build",
     "LGPL-2.1-or-later", "https://ffmpeg.org"),
    ("libbz2", "bzip2", "bzip2-1.0.6", "https://sourceware.org/bzip2/"),
)
elsewhere = {}
still_unknown = []
for name in unaccounted:
    for prefix_, project, license_, source in ELSEWHERE:
        if name.startswith(prefix_):
            elsewhere.setdefault(project, (license_, source))
            break
    else:
        still_unknown.append(name)
unaccounted = still_unknown

with open(os.path.join(out, "BUNDLED-VERSIONS.txt"), "w") as f:
    f.write("Libraries in this bundle, the Homebrew formula each comes from,\n")
    f.write("and the terms Homebrew records for it.\n\n")
    for formula in used:
        version = subprocess.run(["brew", "list", "--versions", formula],
                                 capture_output=True, text=True).stdout.strip()
        f.write(f"{version or formula}\n    {terms.get(formula, 'unknown')}\n")
    if outside:
        f.write("\nBuilt for this bundle rather than taken from Homebrew.\n")
        f.write("Their license texts are in the folders beside this file:\n")
        for project, described in outside:
            f.write(f"    {described}\n")
    if elsewhere:
        f.write("\nTaken from neither Homebrew nor built here. See README.md\n")
        f.write("beside this file for why each one is where it is from:\n")
        for project, (license_, source) in sorted(elsewhere.items()):
            f.write(f"    {project}\n        {license_}\n        {source}\n")
    if unaccounted:
        f.write("\nNot owned by any Homebrew formula (built into TinePlayer\n")
        f.write("itself, or part of macOS):\n")
        for name in unaccounted:
            f.write(f"    {name}\n")

print(f"  {len(used)} formulas, {len(outside)} built here, {taken} license files")
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
in Contents/Frameworks, unmodified.

Everything except GTK is exactly as Homebrew built it. GTK is the same
released version, from the same source, built here with one build option
Homebrew leaves off - `-Daccesskit=enabled`, which compiles in the
accessibility backend that lets a screen reader read this application. No
source was changed; see BUNDLED-VERSIONS.txt for the version and the section
below for the library that option pulls in.

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

## AccessKit

What makes this application readable by VoiceOver. GTK speaks to macOS's
accessibility API through it, and without it a screen reader sees the window
and nothing inside it.

It is dual licensed under the Apache License 2.0 and the MIT license, with
some parts derived from Chromium under a BSD-style license. All three texts
are in the accesskit-c folder beside this file. Its source is at
https://github.com/AccessKit/accesskit-c and
https://github.com/AccessKit/accesskit.

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
