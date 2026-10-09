param([switch]$Force)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Lock = (Get-Content (Join-Path $Root 'external.lock.json') -Raw | ConvertFrom-Json).targets.scripthookv
$Dest = Join-Path $Root 'third_party/scripthookv'
$ReceiptDir = Join-Path $Root 'receipts'
$Receipt = Join-Path $ReceiptDir 'scripthookv.json'
if ((Test-Path (Join-Path $Dest 'inc/main.h')) -and (Test-Path (Join-Path $Dest 'lib/ScriptHookV.lib')) -and -not $Force) {
    Write-Host 'ScriptHookV SDK already present. Use -Force to refresh.'
    exit 0
}
Remove-Item -Recurse -Force $Dest -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Dest,(Join-Path $Dest 'inc'),(Join-Path $Dest 'lib'),(Join-Path $Dest 'runtime'),$ReceiptDir | Out-Null
$temp = Join-Path ([IO.Path]::GetTempPath()) ('skatev-shv-' + [Guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $temp | Out-Null
try {
    $headers = @{ 'User-Agent'='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/154 Safari/537.36'; 'Referer'=$Lock.page }
    $runtimeZip = Join-Path $temp 'runtime.zip'
    $sdkZip = Join-Path $temp 'sdk.zip'
    Invoke-WebRequest -Uri $Lock.runtime_archive -Headers $headers -OutFile $runtimeZip
    Invoke-WebRequest -Uri $Lock.sdk_archive -Headers $headers -OutFile $sdkZip
    $runtimeHash = (Get-FileHash $runtimeZip -Algorithm SHA256).Hash.ToLowerInvariant()
    $sdkHash = (Get-FileHash $sdkZip -Algorithm SHA256).Hash.ToLowerInvariant()
    Expand-Archive $runtimeZip (Join-Path $temp 'runtime') -Force
    Expand-Archive $sdkZip (Join-Path $temp 'sdk') -Force
    $sdkRoot = Get-ChildItem (Join-Path $temp 'sdk') -Directory -Recurse | Where-Object { Test-Path (Join-Path $_.FullName 'inc/main.h') } | Select-Object -First 1
    if (-not $sdkRoot) { $sdkRoot = Get-Item (Join-Path $temp 'sdk') }
    foreach($f in 'main.h','nativeCaller.h','types.h') {
        $src = Get-ChildItem $sdkRoot.FullName -Filter $f -Recurse | Select-Object -First 1
        if (-not $src) { throw "SDK missing $f" }
        Copy-Item $src.FullName (Join-Path $Dest "inc/$f")
    }
    $lib = Get-ChildItem $sdkRoot.FullName -Filter 'ScriptHookV.lib' -Recurse | Select-Object -First 1
    if (-not $lib) { throw 'SDK missing ScriptHookV.lib' }
    Copy-Item $lib.FullName (Join-Path $Dest 'lib/ScriptHookV.lib')
    foreach($f in 'ScriptHookV.dll','dinput8.dll') {
        $src = Get-ChildItem (Join-Path $temp 'runtime') -Filter $f -Recurse | Select-Object -First 1
        if (-not $src) { throw "Runtime missing $f" }
        Copy-Item $src.FullName (Join-Path $Dest "runtime/$f")
    }
    @{
        fetched_utc=(Get-Date).ToUniversalTime().ToString('o')
        declared_version=$Lock.version
        runtime_url=$Lock.runtime_archive
        sdk_url=$Lock.sdk_archive
        runtime_sha256=$runtimeHash
        sdk_sha256=$sdkHash
    } | ConvertTo-Json | Set-Content $Receipt -Encoding UTF8
    Write-Host "Fetched pinned ScriptHookV $($Lock.version). Receipt: $Receipt"
} finally {
    Remove-Item -Recurse -Force $temp -ErrorAction SilentlyContinue
}
