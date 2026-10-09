# Downloads the Python setup runs on into <release>\python: the official
# embeddable Python from python.org plus the numpy and Pillow wheels from PyPI
# (the donor tools' pins), each checked against its published SHA-256 before
# use. About 31 MB; nothing is installed system-wide and the player's own
# Python is never touched. The setup window asks before running this.
#   powershell -ExecutionPolicy Bypass -File tools\get-python.ps1
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Target = Join-Path $Root 'python'
$Pins = @(
  @{ Url = 'https://www.python.org/ftp/python/3.13.15/python-3.13.15-embed-amd64.zip'
     Sha = 'd1f04d990aee1253d8569e8e5104e30fa9f5fa830899f14843448872d936a2cf' },
  @{ Url = 'https://files.pythonhosted.org/packages/cb/3b/d58c12eafcb298d4e6d0d40216866ab15f59e55d148a5658bb3132311fcf/numpy-2.2.6-cp313-cp313-win_amd64.whl'
     Sha = 'b0544343a702fa80c95ad5d3d608ea3599dd54d4632df855e4c8d24eb6ecfa1c' },
  @{ Url = 'https://files.pythonhosted.org/packages/23/85/397c73524e0cd212067e0c969aa245b01d50183439550d24d9f55781b776/pillow-11.3.0-cp313-cp313-win_amd64.whl'
     Sha = '0bce5c4fd0921f99d2e858dc4d4d64193407e1b99478bc5cacecba2311abde51' }
)

Add-Type -AssemblyName System.IO.Compression.FileSystem
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$stage = "$Target.download"
if (Test-Path $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory $stage | Out-Null
$web = New-Object Net.WebClient
foreach ($pin in $Pins) {
  $name = [IO.Path]::GetFileName($pin.Url)
  $file = Join-Path $stage $name
  Write-Host "== python: Downloading $name"
  $web.DownloadFile($pin.Url, $file)
  $sha = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLowerInvariant()
  if ($sha -ne $pin.Sha) { throw "$name does not match its published SHA-256 (got $sha); nothing was installed" }
  # The interpreter goes to the root, the wheels (plain zips) into site-packages.
  $dest = if ($name.EndsWith('.zip')) { "$stage\py" } else { "$stage\py\Lib\site-packages" }
  [IO.Compression.ZipFile]::ExtractToDirectory($file, $dest)
  Remove-Item -LiteralPath $file
}
# Without its ._pth the embeddable interpreter starts like a normal one: the
# script's folder and PYTHONPATH on sys.path (the tools rely on both) and
# Lib\site-packages from site. Setup sets PYTHONNOUSERSITE so packages from the
# player's own Python cannot shadow these.
Remove-Item "$stage\py\python*._pth"
$check = & "$stage\py\python.exe" -s -c "import numpy, PIL, sys; print(sys.version.split()[0], numpy.__version__, PIL.__version__)"
if ($LASTEXITCODE) { throw 'the downloaded Python does not start' }
if (Test-Path $Target) { Remove-Item -LiteralPath $Target -Recurse -Force }
Move-Item -LiteralPath "$stage\py" $Target
Remove-Item -LiteralPath $stage -Recurse -Force
Write-Host "Python ready in $Target (Python, numpy, Pillow: $check)"
