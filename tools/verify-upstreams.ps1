$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Lock = Get-Content (Join-Path $Root 'upstreams.lock.json') -Raw | ConvertFrom-Json
$Base = Join-Path $Root 'upstream'
$failed = $false
foreach ($repo in $Lock.repositories) {
    $dirName = if ($repo.directory) { $repo.directory } else { $repo.name }
    $dest = Join-Path $Base $dirName
    if (-not (Test-Path $dest)) {
        if ($repo.default_fetch) { Write-Error "MISSING $dirName"; $failed = $true }
        else { Write-Host "SKIP $dirName (reference not fetched)" }
        continue
    }
    $head = (& git -C $dest rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $head -ne $repo.commit) {
        Write-Error "BAD $dirName expected=$($repo.commit) actual=$head"
        $failed = $true
    } else {
        Write-Host "OK  $dirName $head"
    }
}
if ($failed) { exit 1 }
