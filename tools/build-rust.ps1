param([ValidateSet('debug','release')][string]$Profile='release')
$ErrorActionPreference='Stop'
$Root=Split-Path -Parent $PSScriptRoot
& (Join-Path $PSScriptRoot 'prepare-skate-overlay.ps1')
if($LASTEXITCODE -ne 0){throw 'Skate overlay preparation failed'}
$Manifest=Join-Path $Root 'rust/Cargo.toml'
# cargo reports warnings/progress on stderr; Windows PowerShell 5.1 would
# turn that into a terminating error under 'Stop'. Exit codes are checked.
$ErrorActionPreference='Continue'
# The runtime, the audio decoder and the setup tools (live_clip, skatev-ped-export, xex_image, skatev-world-cache).
$cargoArgs=@('build','--manifest-path',$Manifest,'-p','skatev-runtime','-p','skate-xma','-p','skatev-ped-export','-p','skatev-world-cache')
if($Profile -eq 'release'){$cargoArgs += '--release'}
& cargo @cargoArgs
if($LASTEXITCODE -ne 0){throw 'Rust runtime build failed'}
$targetDir=if($env:CARGO_TARGET_DIR){$env:CARGO_TARGET_DIR}else{Join-Path $Root 'rust/target'}
if(-not [IO.Path]::IsPathRooted($targetDir)){$targetDir=Join-Path (Get-Location).Path $targetDir}
$src=Join-Path $targetDir "$Profile/SkateVRuntime.dll"
if(-not (Test-Path $src)){throw "Expected Rust DLL missing: $src"}
$out=Join-Path $Root 'build/runtime'
New-Item -ItemType Directory -Force $out|Out-Null
Copy-Item $src (Join-Path $out 'SkateVRuntime.dll') -Force
$package=Join-Path $Root 'build/package'
New-Item -ItemType Directory -Force $package|Out-Null
Copy-Item $src (Join-Path $package 'SkateVRuntime.dll') -Force
$decoder=Join-Path $targetDir "$Profile/skate-xma.exe"
if(-not (Test-Path -LiteralPath $decoder)){throw "Expected audio decoder missing: $decoder"}
$toolsBin=Join-Path $PSScriptRoot 'bin'
New-Item -ItemType Directory -Force $toolsBin | Out-Null
Copy-Item -LiteralPath $decoder -Destination (Join-Path $toolsBin 'skate-xma.exe') -Force
Copy-Item -LiteralPath $decoder -Destination (Join-Path $package 'skate-xma.exe') -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'prepare-skate-audio.py') -Destination $package -Force
# Base default.xex decoder for tools/prepare-audio-tuning.py --xex.
$xexArgs=@('build','--manifest-path',$Manifest,'-p','skatev-ped-export','--bin','xex_image')
if($Profile -eq 'release'){$xexArgs += '--release'}
& cargo @xexArgs
if($LASTEXITCODE -ne 0){throw 'xex_image build failed'}
Copy-Item -LiteralPath (Join-Path $targetDir "$Profile/xex_image.exe") -Destination (Join-Path $toolsBin 'xex_image.exe') -Force
Write-Host "Runtime -> $out"
