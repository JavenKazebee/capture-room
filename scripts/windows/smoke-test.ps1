# Smoke-test the packaged Windows archive the way a user's machine first
# meets it: unpack it into a clean folder, run --version, start the node on a
# temp database and port with only Windows' own folders on PATH, and request
# /api/v1/auth/identity. A DLL or GStreamer plugin missing from the package
# fails here rather than on a user's machine.
#
# Usage: scripts/windows/smoke-test.ps1 <archive>, e.g.
#        scripts/windows/smoke-test.ps1 dist/capture-room-windows-x86_64.tar.gz
# Env:   SMOKE_PORT  the port to start the node on (default 47700)
param([Parameter(Mandatory)][string]$Archive)
$ErrorActionPreference = 'Stop'

$port = if ($env:SMOKE_PORT) { [int]$env:SMOKE_PORT } else { 47700 }
$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$version = (Select-String -Path (Join-Path $repo 'node\Cargo.toml') -Pattern '^version = "(.*)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
$Archive = Resolve-Path $Archive

$work = Join-Path ([IO.Path]::GetTempPath()) "capture-room-smoke-$([guid]::NewGuid())"
New-Item -ItemType Directory $work | Out-Null
$node = $null
try {
    # Windows' own tar: Git for Windows' would read `C:` as a remote host.
    & "$env:SystemRoot\System32\tar.exe" -C $work -xzf $Archive
    if ($LASTEXITCODE) { throw "couldn't unpack $Archive" }
    $exe = Join-Path $work 'Capture Room\bin\capture-room.exe'

    # Not the build's GStreamer: only what the package brings and Windows has.
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
    foreach ($var in 'PKG_CONFIG_PATH', 'GST_PLUGIN_PATH', 'GST_PLUGIN_PATH_1_0', 'GST_PLUGIN_SYSTEM_PATH_1_0') {
        Remove-Item "Env:$var" -ErrorAction SilentlyContinue
    }

    $reported = & $exe --version
    Write-Host $reported
    if ($LASTEXITCODE -or "$reported" -ne "capture-room $version") {
        throw "expected capture-room $version (exit $LASTEXITCODE): a DLL may be missing"
    }

    $log = Join-Path $work 'node.log'
    $node = Start-Process $exe -PassThru -NoNewWindow `
        -ArgumentList '--port', $port, '--db', "`"$(Join-Path $work 'smoke.db')`"" `
        -RedirectStandardOutput $log -RedirectStandardError "$log.err"

    # The first start scans every GStreamer plugin, which can take a minute.
    $url = "http://127.0.0.1:$port/api/v1/auth/identity"
    $deadline = (Get-Date).AddSeconds(180)
    $identity = $null
    while (-not $identity) {
        try {
            $identity = Invoke-RestMethod $url -TimeoutSec 5
        } catch {
            $failure = $null
            if ($node.HasExited) { $failure = "The node exited ($($node.ExitCode)) before it answered $url" }
            elseif ((Get-Date) -gt $deadline) { $failure = "The node didn't answer $url within 180 s" }
            if ($failure) {
                Write-Output "::error::$failure"
                Get-Content $log, "$log.err" -ErrorAction SilentlyContinue
                exit 1
            }
            Start-Sleep -Seconds 1
        }
    }
    $identity | ConvertTo-Json -Compress | Write-Host
    if ($identity.version -ne $version) { throw "expected version $version, got $($identity.version)" }
    Write-Host "Smoke test passed: $(Split-Path -Leaf $Archive)"
} finally {
    if ($node -and -not $node.HasExited) {
        Stop-Process -Id $node.Id -Force
        $node.WaitForExit()
    }
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
