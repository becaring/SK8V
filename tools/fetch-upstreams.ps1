param(
    [switch]$All,
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Lock = Get-Content (Join-Path $Root 'upstreams.lock.json') -Raw | ConvertFrom-Json
$Base = Join-Path $Root 'upstream'
New-Item -ItemType Directory -Force -Path $Base | Out-Null

function Invoke-Git([string[]]$GitArgs) {
    & git @GitArgs
    if ($LASTEXITCODE -ne 0) { throw "git failed: git $($GitArgs -join ' ')" }
}

foreach ($repo in $Lock.repositories) {
    if (-not $All -and -not $repo.default_fetch) { continue }
    $dirName = if ($repo.directory) { $repo.directory } else { $repo.name }
    $dest = Join-Path $Base $dirName
    if (Test-Path $dest) {
        if (-not $Force) {
            $head = (& git -C $dest rev-parse HEAD).Trim()
            if ($LASTEXITCODE -eq 0 -and $head -eq $repo.commit) {
                Write-Host "OK  $dirName @ $head"
                continue
            }
            throw "$dest exists at '$head', expected $($repo.commit). Re-run with -Force to replace it."
        }
        Remove-Item -Recurse -Force $dest
    }
    Write-Host "GET $dirName @ $($repo.commit)"
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    Invoke-Git @('-C', $dest, 'init', '-q')
    Invoke-Git @('-C', $dest, 'remote', 'add', 'origin', $repo.url)
    Invoke-Git @('-C', $dest, 'fetch', '-q', '--depth', '1', 'origin', $repo.commit)
    Invoke-Git @('-C', $dest, '-c', 'advice.detachedHead=false', 'checkout', '-q', '--detach', 'FETCH_HEAD')
    $head = (& git -C $dest rev-parse HEAD).Trim()
    if ($head -ne $repo.commit) { throw "$dirName checked out $head, expected $($repo.commit)" }
}

& (Join-Path $PSScriptRoot 'verify-upstreams.ps1')
