# Build Capture Room for Windows x64 with the official GStreamer MSVC runtime
# beside it, and pack it as dist/capture-room-windows-x86_64.tar.gz
# (+ .sha256).
#
# Usage: scripts/windows/package.ps1
# Env:   GST_DIR           the GStreamer install to build against and bundle
#                          (default C:\gstreamer\1.0\msvc_x86_64; gstreamer.ps1
#                          installs it)
#        MINISIGN_KEY_FILE release key: signs the archive (.minisig), which the
#                          updater requires; needs minisign on PATH
#        SKIP_UI=1         use the existing ui/dist
# Needs cargo-about (`cargo install cargo-about --locked --features cli`) for
# the Rust crates' licenses.
#
# The archive holds one folder, `Capture Room`, which install.ps1 puts at
# %LOCALAPPDATA%\Capture Room\app and the updater swaps in file by file:
#   bin\         capture-room.exe, capture-room-launcher.exe, GStreamer's DLLs
#   lib\         gstreamer-1.0\ (plugins), gio\modules\
#   libexec\     gstreamer-1.0\ (the plugin scanner, the PTP helper)
#   etc\, share\ what GStreamer's libraries read at run time
#   licenses\
#   version.txt  read by the updater, and how the node knows it's installed
$ErrorActionPreference = 'Stop'

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
Set-Location $repo
$gst = if ($env:GST_DIR) { $env:GST_DIR } else { 'C:\gstreamer\1.0\msvc_x86_64' }
$version = (Select-String -Path node\Cargo.toml -Pattern '^version = "(.*)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value

function Check($what) {
    if ($LASTEXITCODE -ne 0) { throw "$what failed" }
}

if (-not (Get-Command cargo-about -ErrorAction SilentlyContinue)) {
    throw 'needs cargo-about: cargo install cargo-about --locked --features cli'
}
if (-not (Test-Path "$gst\bin\gstreamer-1.0-0.dll")) {
    throw "no GStreamer in $gst; install it with scripts/windows/gstreamer.ps1"
}

if (-not $env:SKIP_UI) {
    pnpm install --frozen-lockfile; Check 'pnpm install'
    pnpm build:ui; Check 'the UI build'
}

$env:PKG_CONFIG_PATH = "$gst\lib\pkgconfig"
$env:PATH = "$gst\bin;$env:PATH"
cargo build --release --locked -p capture-room; Check 'cargo build'

$stage = Join-Path $repo 'target\package\windows'
$app = Join-Path $stage 'Capture Room'
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $app | Out-Null

# GStreamer's runtime: the install without what only building needs, its
# debug symbols and tools, and plugins the node never uses (Python, GTK,
# editing, visualizers, DVD, validate) or links in itself.
foreach ($dir in 'bin', 'lib', 'libexec', 'etc', 'share') {
    if (Test-Path "$gst\$dir") { Copy-Item -Recurse "$gst\$dir" $app }
}
$unneeded = 'include', 'lib\pkgconfig', 'lib\gstreamer-1.0\pkgconfig', 'lib\gstreamer-1.0\include',
    'lib\gstreamer-1.0\validate', 'lib\glib-2.0', 'lib\cmake', 'share\gir-1.0', 'share\aclocal',
    'share\gtk-doc', 'share\doc', 'share\man', 'share\vala', 'share\locale', 'share\licenses'
foreach ($path in $unneeded) {
    if (Test-Path "$app\$path") { Remove-Item -Recurse -Force "$app\$path" }
}
Get-ChildItem "$app\lib" -Directory -Filter 'python*' | Remove-Item -Recurse -Force
Get-ChildItem $app -Recurse -File -Include '*.pdb', '*.lib', '*.a', '*.la', '*.h' | Remove-Item -Force
# GLib starts processes (the plugin scanner, the PTP helper) through its
# gspawn helpers, which stay.
Get-ChildItem "$app\bin" -File -Filter '*.exe' | Where-Object Name -NotLike 'gspawn-*' | Remove-Item -Force
$plugins = '^gst(python|gtk.*|ges|nle|libvisual|goom.*|dvdread|resindvd|validate.*|ndi|fallbackswitch|rswebrtc)\.dll$'
Get-ChildItem "$app\lib\gstreamer-1.0" -File | Where-Object Name -Match $plugins | Remove-Item -Force

Copy-Item target\release\capture-room.exe, target\release\capture-room-launcher.exe "$app\bin"
[IO.File]::WriteAllText("$app\version.txt", "$version`n")

# Licenses of the app and of everything in it (THIRD_PARTY_NOTICES.md lists
# them).
$licenses = "$app\licenses"
New-Item -ItemType Directory -Force $licenses | Out-Null
Copy-Item LICENSE, THIRD_PARTY_NOTICES.md $licenses
Copy-Item ui\dist\third-party-licenses.txt "$licenses\ui-packages.txt"
cargo about generate --locked about.hbs -o "$licenses\rust-crates.txt"; Check 'cargo about'
if (-not (Test-Path "$gst\share\licenses")) { throw "no license files in $gst\share\licenses" }
Copy-Item -Recurse "$gst\share\licenses" "$licenses\GStreamer"
if (Test-Path "$gst\share\versions.txt") { Copy-Item "$gst\share\versions.txt" "$licenses\GStreamer" }

# The exe must start with only the DLLs beside it and Windows' own: a
# missing one fails it with no output.
$path = $env:PATH
$env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
$reported = & "$app\bin\capture-room.exe" --version
$code = $LASTEXITCODE
$env:PATH = $path
if ($code -ne 0 -or "$reported" -notmatch [regex]::Escape($version)) {
    throw "the packaged capture-room.exe doesn't start on its own (exit $code): a DLL is missing"
}

New-Item -ItemType Directory -Force dist | Out-Null
$name = 'capture-room-windows-x86_64.tar.gz'
$archive = Join-Path $repo "dist\$name"
# Windows' own tar: Git for Windows' would read `C:` as a remote host.
& "$env:SystemRoot\System32\tar.exe" -C $stage -czf $archive 'Capture Room'; Check 'tar'
$hash = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLower()
[IO.File]::WriteAllText("$archive.sha256", "$hash  $name`n")
if ($env:MINISIGN_KEY_FILE) {
    minisign -S -s $env:MINISIGN_KEY_FILE -m $archive -t "capture-room $version x86_64" | Out-Null
    Check 'minisign'
}
# Only so dist/ can be served as CAPTURE_ROOM_RELEASES to test the updater
# locally; release.yml drops it and its publish job writes the release's.
[IO.File]::WriteAllText("$repo\dist\latest.json", "{`"version`": `"$version`"}`n")
$size = '{0:N0} MB' -f ((Get-Item $archive).Length / 1MB)
Write-Host "Built $archive ($size), version $version"
