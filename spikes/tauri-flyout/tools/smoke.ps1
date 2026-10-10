# Quick smoke run: time-compressed bench + screenshot of ONLY the flyout rect. Kills by exact pid.
param([string]$Mode = "keepalive", [double]$Scale = 0.25, [string]$Theme = "", [string]$Tag = "")
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root "target\release\lastdrop-tauri.exe"
if (-not $Tag) { $Tag = $Mode }
$out = Join-Path $root "smoke\$Tag.json"
$png = Join-Path $root "smoke\$Tag.png"
Remove-Item $out, $png -ErrorAction SilentlyContinue
$argList = @("--bench", $out, "--mode", $Mode, "--bench-scale", "$Scale")
if ($Theme) { $argList += @("--theme", $Theme) }
Add-Type -AssemblyName System.Drawing
$p = Start-Process -FilePath $exe -ArgumentList $argList -PassThru
try {
  # wait for the partial json written at T0 + 12 s * scale, then grab only the flyout rect
  $deadline = (Get-Date).AddSeconds(30 * $Scale + 16)
  $rect = $null
  while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 100
    if (Test-Path $out) {
      try { $j = Get-Content $out -Raw | ConvertFrom-Json; if ($j.flyoutRectPx) { $rect = $j.flyoutRectPx; break } } catch {}
    }
  }
  if ($rect) {
    $pad = 24
    $bmp = [System.Drawing.Bitmap]::new([int]($rect.w + 2*$pad), [int]($rect.h + 2*$pad))
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    Start-Sleep -Milliseconds 400; $g.CopyFromScreen([int]($rect.x - $pad), [int]($rect.y - $pad), 0, 0, $bmp.Size)
    $bmp.Save($png); $g.Dispose(); $bmp.Dispose()
  }
  # process tree of this run (while the flyout is shown)
  $all = Get-CimInstance Win32_Process
  $tree = @($p.Id); $changed = $true
  while ($changed) { $changed = $false; foreach ($q in $all) { if ($tree -contains $q.ParentProcessId -and -not ($tree -contains $q.ProcessId)) { $tree += $q.ProcessId; $changed = $true } } }
  foreach ($q in $all) { if ($tree -contains $q.ProcessId -and $q.ProcessId -ne $p.Id) {
    $type = if ($q.CommandLine -match '--type=([a-z-]+)') { $Matches[1] } else { 'browser' }
    $sub = if ($q.CommandLine -match '--utility-sub-type=([A-Za-z.]+)') { ' ' + $Matches[1] } else { '' }
    Write-Output ("  child {0} pid={1} ppid={2} type={3}{4}" -f $q.Name, $q.ProcessId, $q.ParentProcessId, $type, $sub) } }
  if (-not $p.WaitForExit([int](60000 * $Scale + 20000))) { Write-Output "TIMEOUT - killing $($p.Id)"; Stop-Process -Id $p.Id -Force }
  Write-Output "exit code: $($p.ExitCode)"
} finally {
  if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
  # any of our WebView2 children left behind?
  $left = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($p.Id)" -ErrorAction SilentlyContinue
  Write-Output ("children left: " + (($left | ForEach-Object { "$($_.Name):$($_.ProcessId)" }) -join ", "))
}
