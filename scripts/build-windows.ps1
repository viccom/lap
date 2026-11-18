# Build the three lappp Windows binaries.
#
#   1. lappp.exe         — full desktop app  (links lappp-core: local library,
#                          indexing, AI; the default `embedded` feature)
#   2. lappp-client.exe  — thin client       (no lappp-core; connects to a remote
#                          lappp-server over HTTP; `client` feature)
#   3. lappp-server.exe  — headless server   (the `lappp-server` crate / bin)
#
# Cargo names every binary of a crate after its package, so the two desktop
# variants (embedded vs client) both build as `lappp.exe`; the client copy is
# produced by building with `--features client` and copying the result, rather
# than renaming the Cargo target (which would need a second package).
#
# Usage:  pwsh -File scripts/build-windows.ps1 [-BuildProfile release|debug]
#
#   -BuildProfile release  (default) optimized product build -> build/windows/
#   -BuildProfile debug              dev/test build          -> build/windows/debug/
#
# Release is the default on purpose: debug binaries are for debugging and
# testing, not for shipping. The two profiles use different data directories
# (`com.julyx10.lap` vs `com.julyx10.lap.debug`), so a debug build can never
# clobber the production library — and the outputs never overwrite each other.

param(
    [ValidateSet('release', 'debug')]
    [string]$BuildProfile = 'release'
)

$ErrorActionPreference = 'Stop'

$isRelease = $BuildProfile -eq 'release'
$repo = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $repo 'src-tauri\Cargo.toml'
$outDir = if ($isRelease) {
    Join-Path $repo 'build\windows'
} else {
    Join-Path $repo 'build\windows\debug'
}
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$env:CARGO_HTTP_MULTIPLEXING = 'false'
$env:CARGO_HTTP_VERSION = 'HTTP/1.1'

$cargoArgs = @()
if ($isRelease) { $cargoArgs += '--release' }

function Invoke-Build([string[]]$extra) {
    $cmd = @('build', '--manifest-path', $manifest) + $cargoArgs + $extra
    Write-Host ">> cargo $($cmd -join ' ')"
    & cargo @cmd
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }
}

$targetDir = Join-Path $repo "src-tauri\target\$BuildProfile"

# 1) Full desktop app (embedded backend).
Invoke-Build @('-p', 'lappp')
Copy-Item (Join-Path $targetDir 'lappp.exe') (Join-Path $outDir 'lappp.exe') -Force

# 2) Thin client (no lappp-core; remote lappp-server).
Invoke-Build @('-p', 'lappp', '--no-default-features', '--features', 'client')
Copy-Item (Join-Path $targetDir 'lappp.exe') (Join-Path $outDir 'lappp-client.exe') -Force

# 3) Headless server.
Invoke-Build @('-p', 'lappp-server')
Copy-Item (Join-Path $targetDir 'lappp-server.exe') (Join-Path $outDir 'lappp-server.exe') -Force

Write-Host ''
Write-Host "Built ($BuildProfile) into $outDir :"
Get-ChildItem $outDir -Filter *.exe | Format-Table Name, Length -Auto
