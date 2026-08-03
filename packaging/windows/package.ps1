<#
.SYNOPSIS
    Builds a portable Windows package: TinePlayer with everything it needs.

.DESCRIPTION
    Produces a folder that runs on a machine with no GStreamer, no GTK and no
    Visual C++ runtime beyond what Windows ships. Nothing is installed and
    nothing is written outside it, so it can be unzipped anywhere, run from a
    USB stick, and deleted by deleting the folder.

    What goes in is chosen rather than swept up. The GStreamer installation is
    3.3 GB of plugins, almost none of which a video player needs, and some of
    which cannot be redistributed under this project's terms at all. See
    $Plugins below for what is taken and why.

    Libraries are copied by walking each binary's imports with dumpbin, which
    finds everything that is linked. It does not find libraries opened by name
    at runtime - those are listed in $OpenedByName.

    Produces both a portable ZIP and an installer, and installs Inno Setup to
    build the second if it is not already there.

.PARAMETER GStreamer
    The GStreamer installation to take libraries from. Defaults to the usual
    location of the MSVC 64-bit build.

.PARAMETER Output
    Where to build the package. Defaults to dist/windows at the top of the tree.

.PARAMETER SkipBuild
    Package whatever is already in target/release rather than building first.
#>

param(
    [string]$GStreamer = "$env:LOCALAPPDATA\Programs\gstreamer\1.0\msvc_x86_64",
    [string]$Output,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
# packaging/windows/package.ps1, so the top of the tree is three levels up.
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path))
# Per platform, so three machines can drop their artifacts into one tree
# without colliding, and so a second architecture later has somewhere to go.
if (-not $Output) { $Output = Join-Path $root 'dist\windows' }

# --- What the player needs ---------------------------------------------
#
# Every plugin here is LGPL, BSD, MIT or ISC, which are all terms this
# project can redistribute under. GStreamer's "ugly" set is deliberately
# absent: a52dec (AC-3) and dtsdec (DTS) are GPL, and bundling them would put
# the whole package under the GPL. Their formats still play, because libav
# decodes both under the LGPL.
#
# openh264 is absent for a different reason: Cisco pays the H.264 patent
# licence only for binaries downloaded from Cisco, and that does not extend to
# a copy redistributed here. libav decodes H.264 as well.
$Plugins = @(
    # The pipeline itself.
    'gstcoreelements'       # queue, filesrc, fakesink, the plumbing
    'gstplayback'           # decodebin3, urisourcebin, subtitleoverlay
    'gsttypefindfunctions'  # working out what a file is
    'gstautodetect'         # autoaudiosink, the fallback output

    # Audio, both outputs.
    'gstaudioconvert'
    'gstaudioresample'
    'gstaudiofx'
    'gstvolume'             # the per-output level and mute
    'gstwasapi2'            # Windows audio, and the device list the menu shows
    'gstwavparse'           # the interface's own click sounds

    # Containers. Every one of these is a format someone's library actually
    # contains; leaving one out means a file that will not open at all, with
    # nothing but "missing a plug-in" to say why.
    'gstmatroska'           # MKV, WebM
    'gstisomp4'             # MP4, M4V, MOV
    'gstavi'                # AVI, which older rips are still full of
    'gstogg'                # OGG, OGV
    'gstmpegtsdemux'        # TS, M2TS
    'gstmpegpsdemux'        # MPG, VOB
    'gstasf'                # ASF, WMV
    'gstflv'
    'gstid3demux'
    'gstapetag'
    'gstmxf'                # MXF, which the browser offers
    'gstdv'                 # DV, likewise. NUT comes in through libav.

    # Streaming, for a URL that names a playlist rather than a file.
    'gsthls'
    'gstdash'

    # Parsers, which decodebin needs between demuxer and decoder.
    'gstaudioparsers'
    'gstvideoparsersbad'

    # Decoders that are free of both patent and licence trouble.
    'gstvpx'                # VP8, VP9
    'gstdav1d'              # AV1
    'gstopus'
    'gstvorbis'
    'gstflac'
    'gstalaw'
    'gstmulaw'

    # Everything else, decoded by FFmpeg under the LGPL: H.264, HEVC, AC-3,
    # DTS, AAC, MP3 and the rest of what a film is likely to carry.
    'gstlibav'

    # Video output and colour handling for the GTK sink.
    'gstvideoconvertscale'
    'gstvideofilter'
    'gstopengl'
    'gstd3d11'
    'gstd3d12'

    # Subtitles: text formats, ASS/SSA rendering, and the text drawing they
    # both need.
    'gstsubparse'
    'gstassrender'
    'gstpango'
    'gstclosedcaption'
    # DVD subtitles, from ripped discs. In the "ugly" set, but LGPL in its own
    # right - that set is named for patents, and its plugins declare their own
    # terms. a52dec and dvdreadsrc in the same set declare GPL and are not
    # here; this one does not.
    'gstdvdsub'

    # Playing from a URL, and the TLS to do it over https.
    'gstsoup'
    'gsttcp'
)

# Libraries that nothing imports by name, so walking imports never finds them.
# These are opened at runtime by the plugins above.
$OpenedByName = @(
    'soup-3.0-0.dll'        # by gstsoup, through GIO
    'psl-5.dll'             # by libsoup
    'nghttp2.dll'           # by libsoup
    'sqlite3-0.dll'         # by libsoup's cookie jar
)

Write-Host 'TinePlayer: portable Windows package' -ForegroundColor Cyan

if (-not (Test-Path $GStreamer)) {
    throw "No GStreamer installation at $GStreamer. Pass -GStreamer with its location."
}
$gstBin = Join-Path $GStreamer 'bin'

# --- dumpbin, for walking imports --------------------------------------
# Found through vswhere rather than by guessing a path: the edition, the year
# and the install location all differ between a developer's machine and a
# build runner, and only vswhere knows which is there.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$dumpbin = $null
if (Test-Path $vswhere) {
    $vsRoot = & $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath 2>$null | Select-Object -First 1
    if ($vsRoot) {
        $dumpbin = Get-ChildItem -Path "$vsRoot\VC\Tools\MSVC" -Filter 'dumpbin.exe' `
            -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like '*Hostx64\x64*' } |
        Select-Object -First 1 -ExpandProperty FullName
    }
}
if (-not $dumpbin) {
    # Already on PATH inside a Developer Command Prompt.
    $dumpbin = (Get-Command dumpbin.exe -ErrorAction SilentlyContinue).Source
}
if (-not $dumpbin) {
    throw 'dumpbin.exe not found. It comes with the Visual Studio C++ build tools, which setup-windows.ps1 installs.'
}
Write-Host "  dumpbin from $(Split-Path (Split-Path $dumpbin -Parent) -Parent)" -ForegroundColor DarkGray

# --- Build --------------------------------------------------------------
if (-not $SkipBuild) {
    Write-Host 'Building...' -ForegroundColor Cyan
    Push-Location $root
    try {
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw 'cargo build failed.' }
    } finally {
        Pop-Location
    }
}

$exe = Join-Path $root 'target\release\TinePlayer.exe'
if (-not (Test-Path $exe)) { throw "No executable at $exe." }

$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"' |
    Select-Object -First 1).Matches.Groups[1].Value
$stage = Join-Path $Output "TinePlayer-$version-windows-x64"

if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage -Force | Out-Null
New-Item -ItemType Directory -Path "$stage\lib\gstreamer-1.0" -Force | Out-Null
New-Item -ItemType Directory -Path "$stage\lib\gio\modules" -Force | Out-Null
New-Item -ItemType Directory -Path "$stage\libexec" -Force | Out-Null
New-Item -ItemType Directory -Path "$stage\share\glib-2.0\schemas" -Force | Out-Null
New-Item -ItemType Directory -Path "$stage\licenses" -Force | Out-Null

Copy-Item $exe $stage

# --- Plugins ------------------------------------------------------------
Write-Host "Plugins: $($Plugins.Count) chosen" -ForegroundColor Cyan
$missing = @()
foreach ($plugin in $Plugins) {
    $file = Join-Path $GStreamer "lib\gstreamer-1.0\$plugin.dll"
    if (Test-Path $file) {
        Copy-Item $file "$stage\lib\gstreamer-1.0"
    } else {
        $missing += $plugin
    }
}
if ($missing) {
    Write-Host "  not in this GStreamer, skipped: $($missing -join ', ')" -ForegroundColor Yellow
}

# --- Everything they and the executable link against --------------------
Write-Host 'Walking imports...' -ForegroundColor Cyan

$seen = @{}
$queue = [System.Collections.Queue]::new()
$queue.Enqueue($exe)
Get-ChildItem "$stage\lib\gstreamer-1.0\*.dll" | ForEach-Object { $queue.Enqueue($_.FullName) }
foreach ($name in $OpenedByName) {
    $file = Join-Path $gstBin $name
    if (Test-Path $file) { $queue.Enqueue($file) } else {
        Write-Host "  opened-by-name library absent: $name" -ForegroundColor Yellow
    }
}

while ($queue.Count -gt 0) {
    $binary = $queue.Dequeue()
    $imports = & $dumpbin /dependents $binary 2>$null |
        Select-String -Pattern '^\s{4}(\S+\.dll)$' |
        ForEach-Object { $_.Matches.Groups[1].Value }

    foreach ($import in $imports) {
        $key = $import.ToLowerInvariant()
        if ($seen.ContainsKey($key)) { continue }
        $seen[$key] = $true

        # Anything Windows itself provides. api-ms-win-* and the ucrt are
        # part of the OS on Windows 10 and later.
        if ($key -like 'api-ms-*' -or $key -like 'ext-ms-*' -or $key -eq 'ucrtbase.dll') { continue }

        $source = Join-Path $gstBin $import
        if (-not (Test-Path $source)) { continue }  # a system library

        Copy-Item $source $stage -Force
        $queue.Enqueue((Join-Path $stage $import))
    }
}
$copied = (Get-ChildItem "$stage\*.dll").Count
Write-Host "  $copied libraries" -ForegroundColor Green

# --- The parts that are not libraries -----------------------------------
Copy-Item (Join-Path $GStreamer 'libexec\gstreamer-1.0\gst-plugin-scanner.exe') "$stage\libexec"
Get-ChildItem (Join-Path $GStreamer 'lib\gio\modules\*.dll') | ForEach-Object {
    # gioopenssl carries TLS, which https needs. libproxy reads system proxy
    # settings and is missing its own dependencies in this install anyway.
    if ($_.Name -eq 'gioopenssl.dll') { Copy-Item $_.FullName "$stage\lib\gio\modules" }
}
Copy-Item (Join-Path $GStreamer 'share\glib-2.0\schemas\gschemas.compiled') "$stage\share\glib-2.0\schemas"

# The scanner and the GIO module have their own dependencies.
foreach ($extra in @("$stage\libexec\gst-plugin-scanner.exe", "$stage\lib\gio\modules\gioopenssl.dll")) {
    & $dumpbin /dependents $extra 2>$null |
        Select-String -Pattern '^\s{4}(\S+\.dll)$' |
        ForEach-Object {
            $import = $_.Matches.Groups[1].Value
            $source = Join-Path $gstBin $import
            if ((Test-Path $source) -and -not (Test-Path (Join-Path $stage $import))) {
                Copy-Item $source $stage -Force
            }
        }
}

# --- The paperwork ------------------------------------------------------
Write-Host 'Licenses...' -ForegroundColor Cyan
Copy-Item (Join-Path $root 'LICENSE') "$stage\licenses\TinePlayer-MIT.txt"

# The fonts, beside the executable where use_bundled_fonts looks for them.
# Without these the language menu falls back to whatever the machine has,
# which on Windows draws nothing at all for six of the scripts - the bug they
# exist to fix, and one only visible on a screen most people never open.
Write-Host 'Fonts...' -ForegroundColor Cyan
$fonts = Join-Path $stage 'fonts'
New-Item -ItemType Directory -Path $fonts -Force | Out-Null
Copy-Item (Join-Path $root 'data\fonts\*.ttf') $fonts
Copy-Item (Join-Path $root 'data\fonts\OFL.txt') "$stage\licenses\NotoFonts-OFL.txt"
$count = (Get-ChildItem "$fonts\*.ttf").Count
if ($count -lt 1) { throw 'No fonts were staged. Run packaging/fonts/build-fonts.py first.' }
Write-Host "  $count fonts" -ForegroundColor Green
Copy-Item (Join-Path $root 'THIRD-PARTY.md') $stage
& (Join-Path $PSScriptRoot 'licenses.ps1') -Destination "$stage\licenses" -GStreamer $GStreamer

# --- The portable ZIP ---------------------------------------------------
$zip = Join-Path $Output "TinePlayer-$version-windows-x64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $stage -DestinationPath $zip

# --- The installer ------------------------------------------------------
#
# Both, rather than one or the other. The installer is the front door: it puts
# the application somewhere sensible, gives it a Start Menu entry, and means
# nobody has to find one executable among eighty libraries. The ZIP is for
# people who will not install anything, or who want it on a USB stick.
function Find-Inno {
    @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
        "${env:ProgramFiles}\Inno Setup 6\ISCC.exe"
        # Where winget puts it, since it installs per user by default.
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}

$iscc = Find-Inno
if (-not $iscc) {
    # Installed rather than complained about, the same way the setup scripts
    # treat what they need. It goes in per user, so there is no prompt.
    Write-Host 'Inno Setup 6 is needed to build the installer. Installing it...' -ForegroundColor Cyan
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        winget install --id JRSoftware.InnoSetup --exact --silent `
            --accept-package-agreements --accept-source-agreements | Out-Null
        $iscc = Find-Inno
    } else {
        Write-Host 'winget is not available on this machine.' -ForegroundColor Yellow
    }
}

if ($iscc) {
    Write-Host 'Installer...' -ForegroundColor Cyan
    $iss = Join-Path $PSScriptRoot 'tineplayer.iss'
    & $iscc /Q "/DAppVersion=$version" "/DStageDir=$stage" "/DOutputDir=$Output" "/DRootDir=$root" $iss
    if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed.' }
    $setup = Join-Path $Output "TinePlayer-$version-windows-x64-setup.exe"
} else {
    Write-Host 'No Inno Setup, so no installer was built. The portable ZIP is still there.' -ForegroundColor Yellow
    Write-Host 'Install it by hand with: winget install JRSoftware.InnoSetup' -ForegroundColor Yellow
    $setup = $null
}

$size = '{0:N0} MB' -f ((Get-ChildItem $stage -Recurse | Measure-Object -Property Length -Sum).Sum / 1MB)
Write-Host ''
Write-Host "Packaged  $stage ($size)" -ForegroundColor Green
Write-Host "Zipped    $zip" -ForegroundColor Green
if ($setup) { Write-Host "Installer $setup" -ForegroundColor Green }
Write-Host ''
Write-Host 'Test it on a machine without GStreamer installed, which is the only' -ForegroundColor Yellow
Write-Host 'test that proves it is self-contained.' -ForegroundColor Yellow
