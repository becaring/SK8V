$ErrorActionPreference='Stop'
$Root=Split-Path -Parent $PSScriptRoot
& (Join-Path $PSScriptRoot 'verify-upstreams.ps1')
$Cli=Join-Path $Root 'upstream/rage-cli/Cargo.toml'
$Rpf=(Join-Path $Root 'upstream/rpf-archive-rs').Replace('\\','/')
$Formats=(Join-Path $Root 'upstream/rage-formats').Replace('\\','/')
$Render=(Join-Path $Root 'upstream/rage-render').Replace('\\','/')
if(-not (Test-Path $Cli)){throw 'Missing default upstreams. Run tools/fetch-upstreams.ps1.'}
& cargo build --release --manifest-path $Cli `
  --config "patch.crates-io.rpf-archive.path='$Rpf'" `
  --config "patch.crates-io.rage-formats.path='$Formats'" `
  --config "patch.crates-io.rage-render.path='$Render'"
if($LASTEXITCODE -ne 0){throw 'rage-cli build failed'}
$exe=Join-Path $Root 'upstream/rage-cli/target/release/rage.exe'
if(-not (Test-Path $exe)){throw "rage.exe not found at $exe"}
$out=Join-Path $Root 'build/tools'
New-Item -ItemType Directory -Force $out|Out-Null
Copy-Item $exe (Join-Path $out 'rage.exe') -Force
Write-Host "rage.exe -> $out"
