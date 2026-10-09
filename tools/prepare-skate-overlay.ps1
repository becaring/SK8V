# Materialises the pinned mashup `skate/` workspace into build/overlay/skate and
# applies project-owned patches from patches/skate/*.patch in name order.
# upstream/ stays immutable; the runtime crate depends on the overlay path.
param([switch]$Force)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
& (Join-Path $PSScriptRoot 'verify-upstreams.ps1')
if ($LASTEXITCODE -ne 0) { throw 'upstream verification failed' }

$Source = Join-Path $Root 'upstream/2010-rust-rewrite-mashup/skate'
$Dest = Join-Path $Root 'build/overlay/skate'
$Patches = @(Get-ChildItem (Join-Path $Root 'patches/skate') -Filter '*.patch' -File -ErrorAction SilentlyContinue | Sort-Object Name)
$Stamp = Join-Path $Dest '.skatev-overlay'

$hasher = [Security.Cryptography.SHA256]::Create()
$material = (& git -C (Join-Path $Root 'upstream/2010-rust-rewrite-mashup') rev-parse HEAD).Trim()
foreach ($p in $Patches) { $material += "`n" + $p.Name + ':' + (Get-FileHash $p.FullName -Algorithm SHA256).Hash }
$key = -join ($hasher.ComputeHash([Text.Encoding]::UTF8.GetBytes($material)) | ForEach-Object { $_.ToString('x2') })

if (-not $Force -and (Test-Path $Stamp) -and ((Get-Content $Stamp -Raw).Trim() -eq $key)) {
    Write-Host "Skate overlay up to date ($($Patches.Count) patches)"
    exit 0
}

# Build in a sibling first. A failed patch must never destroy the working
# overlay (it may contain interrupted work not captured in a patch yet).
$overlayParent = [IO.Path]::GetFullPath((Join-Path $Root 'build/overlay'))
$stage = Join-Path $overlayParent ('skate-stage-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item (Join-Path $Source 'Cargo.toml') $stage
Copy-Item (Join-Path $Source 'crates') $stage -Recurse
# The stage sits inside the SkateV repo: stop git from finding it, or its
# eol=lf attributes rewrite upstream's line endings and patches fail.
$env:GIT_CEILING_DIRECTORIES = $overlayParent
foreach ($p in $Patches) {
    Write-Host "PATCH $($p.Name)"
    & git -C $stage apply --whitespace=nowarn -p1 $p.FullName
    if ($LASTEXITCODE -ne 0) { throw "patch failed: $($p.Name); existing overlay preserved; candidate: $stage" }
}
if (Test-Path -LiteralPath $Dest) {
    $uncaptured = @()
    # Only compare source: Cargo may also have written a lock or target tree.
    foreach ($file in @(Get-Item (Join-Path $Dest 'Cargo.toml')) + @(Get-ChildItem (Join-Path $Dest 'crates') -File -Recurse)) {
        $relative = $file.FullName.Substring($Dest.Length + 1)
        $candidate = Join-Path $stage $relative
        # Git may preserve CRLF on context and write LF for inserted lines.
        # Compare source text with only newline encoding normalized; all other
        # differences (including a missing final newline) remain protected.
        if (-not (Test-Path -LiteralPath $candidate) -or
            ([IO.File]::ReadAllText($file.FullName).Replace("`r`n", "`n") -cne
             [IO.File]::ReadAllText($candidate).Replace("`r`n", "`n"))) {
            $uncaptured += $relative
        }
    }
    if ($uncaptured.Count -and -not $Force) {
        throw "Overlay differs from reconstructed patches; capture/review these files before rebuilding: $($uncaptured -join ', '). Existing overlay preserved; candidate: $stage. -Force retains a backup but should only follow review."
    }
    $backup = Join-Path $overlayParent ('skate-backup-' + [Guid]::NewGuid().ToString('N'))
    # Check exact absolute targets before moving a directory recursively.
    foreach ($target in @($Dest, $backup, $stage)) {
        if ([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($target)) -ne $overlayParent) { throw "Unsafe overlay target: $target" }
    }
    Move-Item -LiteralPath $Dest -Destination $backup
    Write-Host "Previous overlay preserved at $backup"
}
Move-Item -LiteralPath $stage -Destination $Dest
Set-Content $Stamp $key -NoNewline
Write-Host "Skate overlay -> $Dest ($($Patches.Count) patches)"
