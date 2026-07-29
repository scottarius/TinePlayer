<#
.SYNOPSIS
    Offers TinePlayer to Kodi as an external player.

.DESCRIPTION
    Writes playercorefactory.xml into Kodi's userdata directory. Kodi has no
    interface for this, so it is otherwise a hand-edited file.

    By default TinePlayer appears under "Play using..." in a video's context
    menu and Kodi goes on playing videos itself. Pass -Default to send every
    video to TinePlayer instead.

    An existing playercorefactory.xml is backed up rather than replaced
    silently, since it may configure other players.

.PARAMETER Default
    Make TinePlayer the player for all video, rather than one option among
    several.

.PARAMETER Userdata
    Kodi's userdata directory, if it is somewhere unusual.
#>

param(
    [switch]$Default,
    [string]$Userdata
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path

# --- The executable Kodi will launch -----------------------------------
$binary = Join-Path $root 'target\release\TinePlayer.exe'
if (-not (Test-Path $binary)) {
    throw "TinePlayer.exe not found at $binary. Build it first with: cargo build --release"
}

# --- Where Kodi keeps its settings -------------------------------------
if (-not $Userdata) {
    $candidates = @(
        "$env:APPDATA\Kodi\userdata",
        "$env:LOCALAPPDATA\Packages\XBMCFoundation.Kodi_4n2hpmxwrvr6p\LocalCache\Roaming\Kodi\userdata"
    )
    $Userdata = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $Userdata) {
    throw 'Could not find Kodi''s userdata directory. Pass -Userdata with its location.'
}

$target = Join-Path $Userdata 'playercorefactory.xml'

# --- Build the file ----------------------------------------------------
$template = Get-Content (Join-Path $root 'data\playercorefactory.xml') -Raw
$xml = $template.Replace('TINEPLAYER_BINARY', $binary)

if (-not $Default) {
    # Drop the rules block, leaving TinePlayer selectable rather than forced.
    $xml = $xml -replace '(?s)<!-- RULES START -->.*?<!-- RULES END -->\r?\n', ''
} else {
    $xml = $xml -replace '<!-- RULES (START|END) -->\r?\n', ''
}

if (Test-Path $target) {
    $backup = "$target.$(Get-Date -Format 'yyyyMMdd-HHmmss').bak"
    Copy-Item $target $backup
    Write-Host "Existing file backed up to $backup" -ForegroundColor Yellow
    Write-Host 'If it configured other players, merge them back by hand.' -ForegroundColor Yellow
}

Set-Content -Path $target -Value $xml -Encoding UTF8
Write-Host "Wrote $target" -ForegroundColor Green
if ($Default) {
    Write-Host 'Kodi will now play all video through TinePlayer.'
} else {
    Write-Host 'In Kodi, use "Play using..." on a video to choose TinePlayer.'
}
Write-Host 'Restart Kodi for it to notice.'
