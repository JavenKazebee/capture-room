# Install the official GStreamer MSVC x64 build (runtime and development
# files) into a folder: what the node builds against, and what package.ps1
# bundles. Without the plugins the node links in itself.
#
# Usage: scripts/windows/gstreamer.ps1 <version> <dir>, e.g.
#        scripts/windows/gstreamer.ps1 1.28.8 C:\gstreamer\1.0\msvc_x86_64
# Does nothing if <dir> already holds that version. Not under Program Files:
# a space in the path breaks the flags pkg-config prints.
param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$Dir
)
$ErrorActionPreference = 'Stop'

$pc = Join-Path $Dir 'lib\pkgconfig\gstreamer-1.0.pc'
if ((Test-Path $pc) -and (Select-String -Path $pc -Pattern "^Version: $([regex]::Escape($Version))$" -Quiet)) {
    Write-Host "GStreamer $Version is already in $Dir"
    exit 0
}

# One installer (since 1.28), runtime and development files.
$exe = "gstreamer-1.0-msvc-x86_64-$Version.exe"
$url = "https://gstreamer.freedesktop.org/data/pkg/windows/$Version/msvc/$exe"
$path = Join-Path ([IO.Path]::GetTempPath()) $exe
Write-Host "Downloading $exe"
curl.exe -fsSL --retry 3 -o $path $url
if ($LASTEXITCODE) { throw "download failed: $url" }
$sum = (curl.exe -fsSL --retry 3 "$url.sha256sum").Split(' ')[0]
if ((Get-FileHash $path -Algorithm SHA256).Hash -ne $sum) { throw "$exe doesn't match its checksum" }

$elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
$scope = if ($elevated) { '/ALLUSERS' } else { '/CURRENTUSER' }
$setup = Start-Process $path -Wait -PassThru -ArgumentList `
    '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', $scope, '/TYPE=devel', "/DIR=$Dir"
if ($setup.ExitCode -ne 0) { throw "the installer exited with $($setup.ExitCode)" }
Remove-Item $path

# Plugins the node links in itself (gst-plugins-rs, statically): the
# install's copies would clash with them.
foreach ($plugin in 'gstndi', 'gstfallbackswitch', 'gstrswebrtc') {
    Remove-Item (Join-Path $Dir "lib\gstreamer-1.0\$plugin.dll") -ErrorAction SilentlyContinue
}
Write-Host "GStreamer $Version installed in $Dir"
