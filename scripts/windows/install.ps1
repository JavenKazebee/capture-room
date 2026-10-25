# Install or remove Capture Room on this PC (Windows 10 or 11, x64), for the
# current user. The app then runs itself as a login service
# (`capture-room service install`) and updates itself.
#
#   irm https://github.com/JavenKazebee/capture-room/releases/latest/download/install.ps1 | iex
#
# With options:
#   & ([scriptblock]::Create((irm https://github.com/JavenKazebee/capture-room/releases/latest/download/install.ps1))) -Controller
#
#   -Controller       make this node a controller (remembered)
#   -Port <n>         API and UI port (default 7700, or what it was)
#   -Version <tag>    install that release instead of the latest, e.g. v0.2.0
#   -Archive <file>   install a local build (capture-room-windows-x86_64.tar.gz)
#   -Uninstall        stop it and remove the app and its logon task; keeps its data
#   -Purge            with -Uninstall, also remove its database and logs
# Recordings in ~\capture-room are never touched.
#
# It runs in the caller's PowerShell session (iex), so it never calls `exit`
# and keeps its variables and functions in a script block of its own.
param(
    [switch]$Controller,
    [int]$Port = 0,
    [string]$Version = '',
    [string]$Archive = '',
    [switch]$Uninstall,
    [switch]$Purge
)

& {
    $ErrorActionPreference = 'Stop'
    $repo = 'JavenKazebee/capture-room'
    $asset = 'capture-room-windows-x86_64.tar.gz'
    $root = Join-Path $env:LOCALAPPDATA 'Capture Room'
    $app = Join-Path $root 'app'
    $exe = Join-Path $app 'bin\capture-room.exe'
    # Windows' own tools: Git for Windows' tar would read `C:` as a remote host.
    $tar = Join-Path $env:SystemRoot 'System32\tar.exe'
    $curl = Join-Path $env:SystemRoot 'System32\curl.exe'

    function Say($text) { Write-Host $text -ForegroundColor Cyan }

    # Move every file under $from to the same place under $to, one rename
    # each: Windows won't delete a running exe or loaded DLL, nor move a folder
    # with one open, but moves the files themselves. All or nothing.
    function Move-Files($from, $to) {
        $moved = @()
        $from = (Get-Item -LiteralPath $from).FullName.TrimEnd('\')
        try {
            foreach ($file in Get-ChildItem -LiteralPath $from -Recurse -File) {
                $dest = Join-Path $to $file.FullName.Substring($from.Length + 1)
                New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
                Move-Item -LiteralPath $file.FullName -Destination $dest
                $moved += , @($file.FullName, $dest)
            }
        } catch {
            [array]::Reverse($moved)
            foreach ($pair in $moved) { Move-Item -LiteralPath $pair[1] -Destination $pair[0] -ErrorAction SilentlyContinue }
            throw
        }
    }

    try {
        $x64 = $env:PROCESSOR_ARCHITECTURE -eq 'AMD64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'AMD64'
        if (-not $x64) { throw 'Capture Room needs 64-bit Windows on an x64 processor.' }

        if ($Uninstall) {
            if (-not (Test-Path $exe)) { throw "Capture Room isn't installed." }
            $serviceArgs = @('service', 'uninstall')
            if ($Purge) { $serviceArgs += '--purge' }
            & $exe @serviceArgs
            if ($LASTEXITCODE -ne 0) { throw 'Uninstalling failed (above).' }
            return
        }

        # Beside the app, on its volume, so moving files in is a rename.
        $work = Join-Path $root ".install-$PID"
        New-Item -ItemType Directory -Force $work | Out-Null
        try {
            $file = Join-Path $work $asset
            if ($Archive) {
                if (-not (Test-Path -LiteralPath $Archive)) { throw "No such file: $Archive" }
                Copy-Item -LiteralPath $Archive $file
            } else {
                if ($Version) {
                    $base = "https://github.com/$repo/releases/download/$Version"
                    Say "Downloading $asset ($Version)"
                } else {
                    $base = "https://github.com/$repo/releases/latest/download"
                    Say "Downloading $asset"
                }
                & $curl -fL --progress-bar -o $file "$base/$asset"
                if ($LASTEXITCODE -ne 0) { throw "Download failed: $base/$asset" }
                & $curl -fsSL -o "$file.sha256" "$base/$asset.sha256"
                if ($LASTEXITCODE -ne 0) { throw "Download failed: $base/$asset.sha256" }
                $expected = ((Get-Content "$file.sha256" -Raw).Trim() -split '\s+')[0]
                if ((Get-FileHash $file -Algorithm SHA256).Hash -ne $expected) {
                    throw 'Checksum mismatch: the download is damaged.'
                }
            }
            & $tar -xzf $file -C $work
            if ($LASTEXITCODE -ne 0) { throw 'Unpacking the archive failed.' }
            $new = Join-Path $work 'Capture Room'
            if (-not (Test-Path (Join-Path $new 'bin\capture-room.exe'))) { throw 'The archive has no Capture Room app.' }

            # A running node keeps its files open: move them aside. The
            # service install below stops it and starts the new one.
            $previous = Join-Path $work 'previous'
            if (Test-Path $app) { Move-Files $app $previous }
            try {
                if (Test-Path $app) { Remove-Item -Recurse -Force $app }
                Move-Item -LiteralPath $new -Destination $app
            } catch {
                if (Test-Path $previous) { Move-Files $previous $app }
                throw
            }

            Say 'Starting Capture Room (the first start can take a minute)'
            $serviceArgs = @('service', 'install')
            if ($Controller) { $serviceArgs += '--controller' }
            if ($Port) { $serviceArgs += '--port', $Port }
            & $exe @serviceArgs
            if ($LASTEXITCODE -ne 0) { throw 'Starting Capture Room failed (above).' }

            # Opens the UI, starting the node first if it isn't running.
            $shell = New-Object -ComObject WScript.Shell
            $shortcut = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath('Programs')) 'Capture Room.lnk'))
            $shortcut.TargetPath = Join-Path $app 'bin\capture-room-launcher.exe'
            $shortcut.WorkingDirectory = $root
            $shortcut.Description = 'Open Capture Room'
            $shortcut.Save()

            Write-Host 'It starts when you sign in, has a tray icon, and is in the Start menu. To remove it:'
            Write-Host "  & ([scriptblock]::Create((irm https://github.com/$repo/releases/latest/download/install.ps1))) -Uninstall"
        } finally {
            # The old node's files stay open a moment after it's stopped.
            for ($i = 0; $i -lt 20 -and (Test-Path $work); $i++) {
                Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
                if (Test-Path $work) { Start-Sleep -Milliseconds 500 }
            }
        }
    } catch {
        Write-Host $_.Exception.Message -ForegroundColor Red
    }
}
