<#
.SYNOPSIS
    Collects the license paperwork a redistributed build has to carry.

.DESCRIPTION
    TinePlayer's own code is MIT, but a packaged build ships other people's
    libraries, and their licenses ask for things in return: the license text
    itself, a notice of what is used, and for the LGPL ones the ability to
    replace them.

    GStreamer's own installation carries per-project license texts and a
    manifest of exact versions, which is better paperwork than anything
    assembled by hand: it describes the precise build being redistributed.
    Those are copied, less the projects deliberately left out of the package.

    Called by package.ps1 beside it; separate because the other platforms
    need the same thing said the same way.
#>

param(
    [Parameter(Mandatory = $true)][string]$Destination,
    [Parameter(Mandatory = $true)][string]$GStreamer
)

$ErrorActionPreference = 'Stop'

# Projects present in the GStreamer installation but deliberately not shipped,
# so their licenses have no business in the package: including them would
# claim this build contains code it does not.
#
# All of these are GPL, patent-encumbered, or both. What they decode is
# handled by FFmpeg under the LGPL instead.
$NotShipped = @(
    'a52dec'        # AC-3, GPL. libav decodes AC-3 under the LGPL.
    'dts'           # DTS, GPL. Likewise.
    'libdca'
    'libdvdread'    # GPL, and nothing here reads a disc.
    'libdvdnav'
    'x264'          # H.264 *encoding*, GPL. Nothing here encodes.
    'x265'
    'mpeg2dec'
    'openh264'      # Cisco's patent licence does not travel with a copy.
    'libmpeg2'
    'rtmpdump'
    'gst-plugins-ugly-1.0'
    'gpl'
)

$source = Join-Path $GStreamer 'share\licenses'
if (-not (Test-Path $source)) {
    throw "No license texts at $source. This GStreamer build cannot be redistributed without them."
}

New-Item -ItemType Directory -Path $Destination -Force | Out-Null

$taken = 0
foreach ($project in Get-ChildItem $source -Directory) {
    if ($NotShipped -contains $project.Name) { continue }
    Copy-Item $project.FullName -Destination $Destination -Recurse -Force
    $taken++
}

# The exact versions of everything, as built. This is what makes the source
# offer below meaningful: without it, "the source for the libraries in this
# package" names no particular thing.
$versions = Join-Path $GStreamer 'share\versions.txt'
if (Test-Path $versions) {
    Copy-Item $versions (Join-Path $Destination 'BUNDLED-VERSIONS.txt')
}

# The notice itself. Written here rather than kept as a file so that it cannot
# drift away from what the script actually does.
$notice = @'
# Licenses

TinePlayer is MIT licensed. Its own terms are in TinePlayer-MIT.txt, and the
Rust libraries compiled into it are listed with their licenses in
THIRD-PARTY.md, beside this folder.

This package also carries libraries it did not write, and this folder holds
their license texts. BUNDLED-VERSIONS.txt records the exact version of each,
as built.

## GStreamer, GTK, GLib and the rest of the runtime

These are free software under the GNU Lesser General Public License, version
2.1 or later. They are shipped here unmodified.

The LGPL asks that you be able to change them and still use this application
with your changed version. You can: they are ordinary shared libraries in this
folder's parent, loaded by name, and replacing one with your own build of the
same version is all that is required. Nothing here is statically linked
against them and nothing checks their contents.

Their source, matching the versions in BUNDLED-VERSIONS.txt, is at:

  GStreamer   https://gitlab.freedesktop.org/gstreamer/gstreamer
  GTK         https://gitlab.gnome.org/GNOME/gtk
  GLib        https://gitlab.gnome.org/GNOME/glib
  FFmpeg      https://git.ffmpeg.org/ffmpeg.git
  libsoup     https://gitlab.gnome.org/GNOME/libsoup

The binaries themselves are built by the GStreamer project's own tooling, and
the same builds can be downloaded from https://gstreamer.freedesktop.org.

## gst-plugin-gtk4

The one library compiled into TinePlayer's executable rather than shipped
beside it, because it is not distributed as a plugin for Windows. It is under
the Mozilla Public License 2.0, which is file-scoped: it does not affect
TinePlayer's own terms, and its source remains available at
https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs.

## What is not here

No GPL-licensed plugin is included. AC-3 and DTS are decoded by FFmpeg under
the LGPL rather than by a52dec and dtsdec, which are GPL. H.264 is decoded by
FFmpeg for the same reason, rather than by openh264, whose patent licence
covers only binaries obtained from Cisco.

Patents are a separate matter from copyright, and vary by country. This
package decodes what the libraries above decode; whether a particular format
needs a patent licence where you are is not something this file can answer.
'@
Set-Content -Path (Join-Path $Destination 'README.md') -Value $notice -Encoding UTF8

Write-Host "  $taken license folders, plus versions and notice" -ForegroundColor Green
