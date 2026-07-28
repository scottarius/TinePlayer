<#
.SYNOPSIS
    Installs everything TinePlayer needs to build and run on Windows.

.DESCRIPTION
    The Windows counterpart to install.sh. Every step is skipped if already
    satisfied, so this is safe to re-run.

    Installs:
      - Rust (MSVC toolchain)
      - Visual Studio 2022 Build Tools with the C++ workload, for the linker
        that Rust's MSVC toolchain needs
      - GStreamer (MSVC build)

    GStreamer's Windows distribution bundles GTK 4, glib, cairo, pango and
    the rest of the GTK stack alongside GStreamer itself, so it supplies
    every native dependency this project has. Do not install GTK separately
    (for example with gvsbuild): a second GTK brings a second copy of glib,
    and whichever one wins on PKG_CONFIG_PATH gets mixed with the other's
    headers and build tools. That combination fails to build, and would be
    a latent crash risk at runtime if it did.

    Everything must be built for MSVC to match Rust's MSVC toolchain, so
    avoid MSYS2/MinGW builds of either library.

.NOTES
    Run in a normal PowerShell prompt; winget elevates on its own where
    needed. Open a new terminal afterwards so the environment variables
    this sets are picked up.
#>

$ErrorActionPreference = 'Stop'

function Test-Command($Name) {
    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

# Where GStreamer landed is not a constant: winget installs per-user by
# default, but a machine-scope or elevated install goes somewhere else
# entirely. The installer records its own prefix in
# GSTREAMER_1_0_ROOT_MSVC_X86_64, so that is asked first and the known
# defaults are only a fallback. Read from the registry rather than the
# current process, because a variable set by an install in this same run
# won't be in this process's environment yet.
function Resolve-GstRoot {
    $candidates = @(
        [Environment]::GetEnvironmentVariable('GSTREAMER_1_0_ROOT_MSVC_X86_64', 'User')
        [Environment]::GetEnvironmentVariable('GSTREAMER_1_0_ROOT_MSVC_X86_64', 'Machine')
        $env:GSTREAMER_1_0_ROOT_MSVC_X86_64
        "$env:LOCALAPPDATA\Programs\gstreamer\1.0\msvc_x86_64"
        'C:\gstreamer\1.0\msvc_x86_64'
    )
    foreach ($candidate in $candidates) {
        if (-not $candidate) { continue }
        # The environment variable carries a trailing backslash; leaving it
        # in would produce a doubled separator in PKG_CONFIG_PATH.
        $root = $candidate.TrimEnd('\')
        if (Test-Path "$root\lib\pkgconfig\gtk4.pc") { return $root }
    }
    return $null
}

# Pull PATH back out of the registry so tools installed by winget earlier
# in this same run are usable without opening a new terminal.
function Update-SessionPath {
    $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $user = [Environment]::GetEnvironmentVariable('Path', 'User')
    $extra = "$env:USERPROFILE\.cargo\bin"
    $env:Path = (@($machine, $user, $extra) | Where-Object { $_ }) -join ';'
}

function Install-WingetPackage($Id, $Label, $ExtraArgs = @()) {
    Write-Host "Installing $Label..." -ForegroundColor Cyan
    $wingetArgs = @(
        'install', '--id', $Id, '-e', '--source', 'winget',
        '--accept-package-agreements', '--accept-source-agreements'
    ) + $ExtraArgs
    & winget @wingetArgs
    # winget exits non-zero when a package is already installed or has no
    # available update; neither is a failure here.
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  winget exit $LASTEXITCODE (already installed, or no update available)" -ForegroundColor DarkGray
    }
    Update-SessionPath
}

if (-not (Test-Command 'winget')) {
    throw 'winget is required but not available. Install "App Installer" from the Microsoft Store, then re-run.'
}

# --- Rust -------------------------------------------------------------
if (Test-Command 'cargo') {
    Write-Host 'Rust already installed, skipping.' -ForegroundColor DarkGray
} else {
    Install-WingetPackage 'Rustlang.Rustup' 'Rust'
    rustup default stable-msvc
}

# --- Visual Studio C++ build tools ------------------------------------
# Rust's MSVC toolchain shells out to link.exe, which ships with these.
# Matches any edition (BuildTools, Community, Professional, Enterprise).
$msvcGlobs = @(
    "${env:ProgramFiles}\Microsoft Visual Studio\*\*\VC\Tools\MSVC",
    "${env:ProgramFiles(x86)}\Microsoft Visual Studio\*\*\VC\Tools\MSVC"
)
$hasMsvc = $msvcGlobs | ForEach-Object {
    Get-ChildItem $_ -Directory -ErrorAction SilentlyContinue
} | Select-Object -First 1

if ($hasMsvc) {
    Write-Host 'Visual Studio C++ build tools already installed, skipping.' -ForegroundColor DarkGray
} else {
    Install-WingetPackage 'Microsoft.VisualStudio.2022.BuildTools' 'Visual Studio 2022 Build Tools (C++)' @(
        '--override',
        '--wait --quiet --add ProductLang En-us --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
    )
}

# --- GStreamer (also supplies GTK 4) ----------------------------------
$GstRoot = Resolve-GstRoot
if ($GstRoot) {
    Write-Host "GStreamer already installed at $GstRoot, skipping." -ForegroundColor DarkGray
} else {
    Install-WingetPackage 'gstreamerproject.gstreamer' 'GStreamer (MSVC)'
    $GstRoot = Resolve-GstRoot
    if (-not $GstRoot) {
        throw 'GStreamer was installed but could not be located afterwards. Set GSTREAMER_1_0_ROOT_MSVC_X86_64 to its install directory and re-run.'
    }
}

foreach ($pc in 'gstreamer-1.0', 'gtk4', 'glib-2.0') {
    if (-not (Test-Path "$GstRoot\lib\pkgconfig\$pc.pc")) {
        throw "$pc.pc not found under $GstRoot. Make sure the MSVC (not MinGW) GStreamer package is installed, and that it includes development files."
    }
}

# --- Environment ------------------------------------------------------
# Deliberately points at the GStreamer prefix only. Adding a second prefix
# that also ships GTK or glib is what breaks the build.
Write-Host 'Setting environment variables...' -ForegroundColor Cyan

[Environment]::SetEnvironmentVariable('PKG_CONFIG_PATH', "$GstRoot\lib\pkgconfig", 'User')

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$GstRoot\bin*") {
    $combined = (@("$GstRoot\bin") + @($userPath | Where-Object { $_ })) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $combined, 'User')
}

Write-Host ''
Write-Host 'Done. Open a new terminal (so the environment changes apply), then:' -ForegroundColor Green
Write-Host '    cargo build --release'
Write-Host '    .\target\release\TinePlayer.exe'
