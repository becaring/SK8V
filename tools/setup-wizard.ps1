# SK8V setup window: picks the Skate 3 disc and the GTA V folder, then runs tools/prepare.py --install. There is no uninstaller:
# removing SK8V is deleting its loose files (README).
# Windows PowerShell 5.1 + WinForms, nothing to install; the release's
# "SK8V Setup.cmd" starts it. ASCII only: 5.1 reads a BOM-less script as ANSI.
# -Disc/-Folder/-Ride/-Start: the restart as administrator carries the choices over and goes straight on.
# (Not -Skate/-Gta/-Stance: PowerShell names ignore case, and those are the window's controls.)
param([string]$Disc = '', [string]$Folder = '', [string]$Ride = 'Regular', [switch]$Start)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
[Windows.Forms.Application]::EnableVisualStyles()
$Root = Split-Path -Parent $PSScriptRoot
$Self = $PSCommandPath

# The copy tools\get-python.ps1 downloaded, else a python.exe on PATH (not the Microsoft Store stub)
# that is 3.11+ with numpy and Pillow and whose DLL-backed modules load (a player's Anaconda Python run
# outside its environment had numpy but no ctypes: "DLL load failed while importing _ctypes"). $null when there is none.
function Find-Python {
  $bundled = Join-Path $Root 'python\python.exe'
  if (Test-Path $bundled) { return $bundled }
  $ErrorActionPreference = 'Continue'  # under Stop, 5.1 turns the probe's redirected stderr into an exception
  Get-Command python.exe -CommandType Application -All -ErrorAction SilentlyContinue |
    Where-Object { $_.Source -notmatch '\\WindowsApps\\' } | ForEach-Object Source |
    Where-Object { & $_ -c 'import sys, ctypes, hashlib, zlib, xml.etree.ElementTree, numpy, PIL.Image; sys.exit(sys.version_info < (3, 11))' 2>$null; $LASTEXITCODE -eq 0 } |
    Select-Object -First 1
}

function Find-Gta {
  $keys = 'HKLM:\SOFTWARE\WOW6432Node\Rockstar Games\Grand Theft Auto V',
          'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Steam App 271590',
          'HKLM:\SOFTWARE\WOW6432Node\Rockstar Games\GTAV'
  foreach ($k in $keys) {
    $p = Get-ItemProperty $k -ErrorAction SilentlyContinue
    foreach ($v in $p.InstallFolder, $p.InstallLocation) {
      if ($v -and (Test-Path (Join-Path $v 'GTA5.exe'))) { return $v }
    }
  }
  ''
}

# ---- look ----
$rgb = { param($r, $g, $b) [Drawing.Color]::FromArgb($r, $g, $b) }
$accent = & $rgb 255 92 0; $ink = & $rgb 28 28 32; $muted = & $rgb 110 110 120; $track = & $rgb 226 226 232
function New-Font([float]$size, [string]$style = 'Regular') { New-Object Drawing.Font('Segoe UI', $size, [Drawing.FontStyle]$style) }
function Add([string]$type, [hashtable]$props, $parent = $form) {
  $c = New-Object "Windows.Forms.$type" -Property $props
  $parent.Controls.Add($c)
  $c
}
function Set-Flat($button, $back, $fore) {
  $button.FlatStyle = 'Flat'; $button.BackColor = $back; $button.ForeColor = $fore
  $button.FlatAppearance.BorderColor = $back; $button.Cursor = [Windows.Forms.Cursors]::Hand
}

$form = New-Object Windows.Forms.Form -Property @{
  Text = 'SK8V Setup'; ClientSize = '640,420'; StartPosition = 'CenterScreen'; FormBorderStyle = 'FixedSingle'
  MaximizeBox = $false; Font = (New-Font 9.5); BackColor = (& $rgb 247 247 249); ForeColor = $ink
}
$header = Add Panel @{ Location = '0,0'; Size = '640,96'; BackColor = (& $rgb 20 20 24) }
[void](Add Label @{ Text = 'SK8V'; Location = '22,14'; AutoSize = $true; Font = (New-Font 24 'Bold'); ForeColor = 'White' } $header)
[void](Add Label @{ Text = 'Skate 3 in GTA V Story Mode'; Location = '26,60'; AutoSize = $true; ForeColor = (& $rgb 170 170 180) } $header)
[void](Add Panel @{ Dock = 'Bottom'; Height = 3; BackColor = $accent } $header)

$y = 118
function Add-Row($label, $value, $browse) {
  [void](Add Label @{ Text = $label; Location = "24,$($script:y + 4)"; Size = '140,22' })
  $t = Add TextBox @{ Text = $value; Location = "164,$script:y"; Size = '346,26' }
  $b = Add Button @{ Text = 'Browse...'; Location = "518,$($script:y - 1)"; Size = '98,28' }
  Set-Flat $b (& $rgb 232 232 238) $ink
  $b.Add_Click({ & $browse $t }.GetNewClosure())
  $script:y += 38
  $t
}
$pickFile = { param($t) $d = New-Object Windows.Forms.OpenFileDialog -Property @{ Filter = 'Xbox 360 disc image (*.iso)|*.iso|All files|*.*' }
              if ($d.ShowDialog() -eq 'OK') { $t.Text = $d.FileName } }
$pickDir = { param($t) $d = New-Object Windows.Forms.FolderBrowserDialog
             if ($d.ShowDialog() -eq 'OK') { $t.Text = $d.SelectedPath } }

$skate = Add-Row 'Skate 3 disc (ISO)' $Disc $pickFile
$skateDir = Add LinkLabel @{ Text = 'or pick an extracted disc folder'; Location = "164,$($y - 10)"; AutoSize = $true; LinkColor = $muted; Font = (New-Font 8.5) }
$skateDir.Add_LinkClicked({ & $pickDir $skate })
$y += 14
$gta = Add-Row 'GTA V Legacy folder' $(if ($Folder) { $Folder } else { Find-Gta }) $pickDir
[void](Add Label @{ Text = 'Stance'; Location = "24,$($y + 4)"; Size = '140,22' })
$stance = Add ComboBox @{ Location = "164,$y"; Size = '120,26'; DropDownStyle = 'DropDownList'; FlatStyle = 'Flat' }
[void]$stance.Items.AddRange(@('Regular', 'Goofy')); $stance.SelectedIndex = [int]($Ride -eq 'Goofy')
$y += 40
[void](Add Label @{ Location = "24,$y"; Size = '592,52'; ForeColor = $muted; Font = (New-Font 8.75)
  Text = "Needs GTA V Legacy 1.0.3889.0 with ScriptHookV, your own Skate 3 (Xbox 360) disc image and about 7 GB free on the GTA drive (about 2 GB stays, in the GTA folder's SK8V folder). Close GTA V first. Setup can take several minutes, longer on slower PCs and hard drives; if it stops, press Install again and it picks up where it left off." })
$y += 62

# Progress: the current step, a bar, percent and elapsed time. The result panel covers it at the end.
$status = Add Label @{ Text = 'Ready when you are.'; Location = "24,$y"; Size = '592,22'; Font = (New-Font 10 'Bold') }
$bar = Add Panel @{ Location = "24,$($y + 28)"; Size = '592,10'; BackColor = $track }
$fill = Add Panel @{ Location = '0,0'; Size = '0,10'; BackColor = $accent } $bar
$meta = Add Label @{ Location = "24,$($y + 42)"; Size = '592,20'; ForeColor = $muted; Font = (New-Font 8.75) }
$result = Add Panel @{ Location = "24,$($y - 4)"; Size = '592,70'; Visible = $false }
$resultTitle = Add Label @{ Location = '14,8'; Size = '570,26'; Font = (New-Font 12 'Bold') } $result
$resultText = Add Label @{ Location = '14,36'; Size = '570,32'; Font = (New-Font 9) } $result
$y += 84

$details = Add LinkLabel @{ Text = 'Show details'; Location = "24,$($y + 9)"; AutoSize = $true; LinkColor = $muted }
$install = Add Button @{ Text = 'Install'; Location = "396,$y"; Size = '120,36'; Font = (New-Font 10 'Bold') }
Set-Flat $install $accent 'White'
$close = Add Button @{ Text = 'Close'; Location = "526,$y"; Size = '90,36' }
Set-Flat $close (& $rgb 232 232 238) $ink
$close.Add_Click({ $form.Close() })
$y += 50
$log = Add TextBox @{ Multiline = $true; ScrollBars = 'Vertical'; ReadOnly = $true; Location = "24,$y"; Size = '592,200'
  Font = (New-Object Drawing.Font('Consolas', 8.5)); BackColor = 'White'; Visible = $false }
function Show-Details([bool]$show) {
  $log.Visible = $show
  $details.Text = $(if ($show) { 'Hide details' } else { 'Show details' })
  $form.ClientSize = New-Object Drawing.Size(640, ($log.Top + $(if ($show) { 216 } else { 0 })))
}
Show-Details $false
$details.Add_LinkClicked({ Show-Details (-not $log.Visible) })

function Show-Result([bool]$ok, [string]$title, [string]$text) {
  $result.BackColor = $(if ($ok) { & $rgb 230 246 234 } else { & $rgb 253 234 232 })
  $resultTitle.ForeColor = $(if ($ok) { & $rgb 22 120 52 } else { & $rgb 180 30 20 })
  $resultTitle.Text = $title; $resultText.Text = $text
  $status.Visible = $bar.Visible = $meta.Visible = $false
  $result.Visible = $true
}
function Set-Running([bool]$running) {
  $install.Enabled = -not $running
  $install.Text = $(if ($running) { 'Installing...' } else { 'Install' })
  Set-Flat $install $(if ($running) { & $rgb 200 200 208 } else { $accent }) 'White'
}
function Set-Progress([int]$percent) { $fill.Width = [int]($bar.Width * [Math]::Min(100, $percent) / 100); $script:percent = $percent }

# ---- running a step ----
# The tool writes to a log file the timer tails (PowerShell event handlers would not run while the window is open).
# prepare.py prints '== <stage>: <what it does>' as each stage starts and '@progress <percent>' as each ends.
$logFile = Join-Path $env:TEMP 'sk8v-setup.log'
$script:proc = $null
$script:then = $null  # what starts when the running step succeeds
$script:read = 0
$script:partial = ''
$script:lastLine = ''
$script:percent = 0
$timer = New-Object Windows.Forms.Timer -Property @{ Interval = 250 }
$timer.Add_Tick({
  if (-not $script:proc) { return }
  try {
    $fs = [IO.File]::Open($logFile, 'Open', 'Read', 'ReadWrite')
    try {
      [void]$fs.Seek($script:read, 'Begin')
      $buf = New-Object byte[] ($fs.Length - $script:read)
      $n = $fs.Read($buf, 0, $buf.Length)
      $script:read += $n
      $lines = ($script:partial + [Text.Encoding]::UTF8.GetString($buf, 0, $n)) -split "`r?`n"
      $script:partial = $lines[-1]
      foreach ($line in $lines[0..($lines.Count - 2)]) {
        if ($line -match '^@progress (\d+)') { Set-Progress ([int]$Matches[1]); continue }
        if ($line -match '^== [^:]+: (.+)') { $status.Text = $Matches[1] + '...' }
        if ($line.Trim()) { $script:lastLine = $line.Trim() }
        $log.AppendText($line + "`r`n")
      }
    } finally { $fs.Dispose() }
  } catch [IO.FileNotFoundException] {}
  $elapsed = (Get-Date) - $script:started
  $meta.Text = '{0}%   |   {1:m\:ss} elapsed' -f $script:percent, $elapsed
  if ($script:proc.HasExited) {
    $code, $then = $script:proc.ExitCode, $script:then
    $script:proc, $script:then = $null, $null
    if ($code -eq 0 -and $then) { & $then; return }
    Set-Running $false
    if ($code -eq 0) {
      Set-Progress 100
      $status.Text = 'Done'
      Show-Result $true 'SK8V is installed' ("Start GTA V Story Mode with BattlEye off. Pick the skateboard in the weapon wheel to skate; " +
                                             "/ (or Back + LB) opens the SK8V menu. Took {0:m\:ss}." -f $elapsed)
      $install.Visible = $false
      $close.Text = 'Finish'; Set-Flat $close $accent 'White'; $close.Font = (New-Font 10 'Bold')
    } else {
      $status.Text = 'Setup stopped'
      Show-Result $false 'Setup stopped' ("$($script:lastLine)`nNothing in the GTA folder was changed. Fix that and press Install to pick up where it left off.")
      $install.Text = 'Try again'
      Show-Details $true
    }
  }
})
$timer.Start()

# Runs a command (each word quoted) with its output in the log file the timer shows; $then runs if it succeeds.
function Start-Logged([string[]]$words, [scriptblock]$then) {
  Remove-Item $logFile -ErrorAction SilentlyContinue
  $script:read = 0; $script:partial = ''; $script:then = $then
  # a trailing backslash would escape the closing quote (F:\ -> F:)
  $command = ($words | ForEach-Object { '"' + ([string]$_).TrimEnd('\') + '"' }) -join ' '
  $script:proc = Start-Process cmd.exe -ArgumentList "/c `"$command > `"$logFile`" 2>&1`"" -WorkingDirectory $Root `
    -WindowStyle Hidden -PassThru
  [void]$script:proc.Handle  # keeps ExitCode readable after the process exits
  Set-Running $true
}

# A real write (as setup_engine.writable): the folder's permissions, not its read-only flag.
function Test-Writable([string]$dir) {
  $probe = Join-Path $dir ".sk8v-write-test-$PID"
  try { [IO.File]::WriteAllBytes($probe, @()); Remove-Item -LiteralPath $probe; $true } catch { $false }
}

function Start-Prepare {
  $py = Find-Python
  if (-not $py) { Show-Result $false 'No usable Python' 'Setup needs Python 3.11+ with numpy and Pillow.'; Set-Running $false; return }
  $env:PYTHONIOENCODING = 'utf-8'
  # The downloaded copy ignores the user's site-packages (they may hold other numpy/Pillow versions);
  # the player's own Python keeps them, since that is often where pip --user put numpy and Pillow.
  $env:PYTHONNOUSERSITE = $(if ($py -eq (Join-Path $Root 'python\python.exe')) { '1' } else { $null })
  $status.Text = 'Starting...'
  Start-Logged @($py, '-B', '-u', (Join-Path $Root 'tools\prepare.py'), '--skate', $skate.Text,
                 '--gta', $gta.Text, '--stance', $stance.SelectedItem, '--install', '--clean')
}

$install.Add_Click({
  if (-not $skate.Text -or -not (Test-Path $skate.Text)) { [void][Windows.Forms.MessageBox]::Show('Pick your Skate 3 disc image.', 'SK8V Setup'); return }
  if (-not (Test-Path (Join-Path $gta.Text 'GTA5.exe'))) { [void][Windows.Forms.MessageBox]::Show('Pick the folder that holds GTA5.exe.', 'SK8V Setup'); return }
  $result.Visible = $false
  $status.Visible = $bar.Visible = $meta.Visible = $true
  Set-Progress 0
  $script:started = Get-Date
  if (-not (Test-Writable $gta.Text)) {
    $ask = "Windows only lets administrators change this GTA V folder (a GTA download or repair sets that).`n`n" +
           "Restart setup as administrator? Your choices carry over."
    if ([Windows.Forms.MessageBox]::Show($ask, 'SK8V Setup', 'YesNo', 'Question') -ne 'Yes') { return }
    $words = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $Self, '-Disc', $skate.Text, '-Folder', $gta.Text,
               '-Ride', $stance.SelectedItem, '-Start') | ForEach-Object { '"' + ([string]$_).TrimEnd('\') + '"' }
    try { # not -WindowStyle Hidden: Windows applies that to this window's first show as well
      Start-Process powershell.exe -Verb RunAs -ArgumentList ($words -join ' ')
      $form.Close()
    } catch {} # the Windows prompt was declined: stay here
    return
  }
  if (Find-Python) { Start-Prepare; return }
  $ask = "Setup runs on Python 3.13 with numpy and Pillow, which this PC does not have.`n`n" +
         "Download them now? About 31 MB from python.org and PyPI, checked against their published hashes and " +
         "kept in this setup folder. Nothing is installed on your PC."
  if ([Windows.Forms.MessageBox]::Show($ask, 'SK8V Setup', 'YesNo', 'Question') -ne 'Yes') { return }
  $status.Text = 'Downloading Python...'
  Start-Logged @('powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $Root 'tools\get-python.ps1')) { Start-Prepare }
})

# Closing mid-run stops the tools (cmd, Python and the extractors under it); Install resumes later.
$form.Add_FormClosing({
  if (-not $script:proc -or $script:proc.HasExited) { return }
  $q = "Setup is still running. Stop it? Pressing Install later picks up where it stopped."
  if ([Windows.Forms.MessageBox]::Show($q, 'SK8V Setup', 'YesNo', 'Warning') -ne 'Yes') { $_.Cancel = $true; return }
  Start-Process taskkill.exe -ArgumentList "/T /F /PID $($script:proc.Id)" -WindowStyle Hidden -Wait
})
if ($Start) { $form.Add_Shown({ $install.PerformClick() }) }
[void]$form.ShowDialog()
