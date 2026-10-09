param([ValidateSet('Debug','Release')][string]$Config='Release')
$ErrorActionPreference='Stop'
$Root=Split-Path -Parent $PSScriptRoot
if(-not (Test-Path (Join-Path $Root 'third_party/scripthookv/inc/main.h'))){throw 'Run tools/fetch-scripthookv.ps1 first.'}
Push-Location $Root
try {
  & cmake --preset vs2022-x64
  if($LASTEXITCODE -ne 0){throw 'CMake configure failed'}
  & cmake --build (Join-Path $Root 'build/host') --config $Config
  if($LASTEXITCODE -ne 0){throw 'Host build failed'}
} finally { Pop-Location }
$asi=Join-Path $Root "build/host/host/$Config/SkateVLegacy.asi"
if(-not (Test-Path -LiteralPath $asi)){throw "SkateVLegacy.asi not found after build: $asi"}
$out=Join-Path $Root 'build/package'
New-Item -ItemType Directory -Force $out|Out-Null
Copy-Item -LiteralPath $asi -Destination (Join-Path $out 'SkateVLegacy.asi') -Force
$runtime=Join-Path $Root 'build/runtime/SkateVRuntime.dll'
if(Test-Path $runtime){Copy-Item $runtime (Join-Path $out 'SkateVRuntime.dll') -Force}
Write-Host "Host package -> $out"
