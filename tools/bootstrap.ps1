# Fetches the pinned sources and builds everything the release needs:
# the Rust runtime and tools, the ASI and rage.exe.
#   powershell -ExecutionPolicy Bypass -File tools\bootstrap.ps1
$ErrorActionPreference='Stop'
& (Join-Path $PSScriptRoot 'fetch-upstreams.ps1')
& (Join-Path $PSScriptRoot 'fetch-scripthookv.ps1')
& (Join-Path $PSScriptRoot 'build-rust.ps1')
& (Join-Path $PSScriptRoot 'build-host.ps1')
& (Join-Path $PSScriptRoot 'build-rage-cli.ps1')
Write-Host 'Built. Next: python tools\build-release.py --version <version>'
