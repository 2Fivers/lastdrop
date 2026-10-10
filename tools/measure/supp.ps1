param([string]$Mode = 'keepalive', [string]$Tag = 'supp-tauri-keepalive')
$SPIKE = (Resolve-Path (Join-Path $PSScriptRoot '..\..\spikes')).Path
$D = (Join-Path $PSScriptRoot 'results\supp'); New-Item -ItemType Directory -Force $D | Out-Null
if (-not ('LdMeasure' -as [type])) { Add-Type -Path (Join-Path $PSScriptRoot 'LdMeasure.cs') }
Add-Type -AssemblyName System.Drawing
[void][LdMeasure]::SetProcessDpiAwarenessContext([IntPtr](-4)); [void][LdMeasure]::SetThreadDpiAwarenessContext([IntPtr](-4))
$busy = Get-Process | Where-Object { $_.Name -match '^(cargo|rustc|link|lld-link|cl|lastdrop-.*)$' }
if ($busy) { "BUSY: $($busy.Name -join ',')"; exit 1 }
$out = "$D\$Tag.json"; Remove-Item $out -ErrorAction SilentlyContinue
$p = Start-Process "$SPIKE\tauri-flyout\target\release\lastdrop-tauri.exe" -ArgumentList "--bench `"$out`" --mode $Mode --bench-scale 0.4" -PassThru
# show at 4 s, partial json at 4.8 s
Start-Sleep -Milliseconds 6000
$all = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name, CommandLine
$tree = @{ $p.Id = $true }; $ch = $true
while ($ch) { $ch = $false; foreach ($w in $all) { if (-not $tree.ContainsKey([int]$w.ProcessId) -and $tree.ContainsKey([int]$w.ParentProcessId)) { $tree[[int]$w.ProcessId] = $true; $ch = $true } } }
$perf = (New-Object System.Diagnostics.PerformanceCounterCategory 'Process').ReadCategory()
$pmap = @{}; foreach ($i in $perf['ID Process'].Values) { $pmap[[int]$i.RawValue] = $perf['Working Set - Private'][$i.InstanceName].RawValue }
foreach ($w in $all | Where-Object { $tree.ContainsKey([int]$_.ProcessId) }) {
    $type = if ($w.CommandLine -match '--type=([\w-]+)') { $Matches[1] } else { 'browser/exe' }
    $svc = if ($w.CommandLine -match '--utility-sub-type=([\w.]+)') { $Matches[1] } else { '' }
    '{0,-8} {1,-8} {2,-20} {3,-16} {4,-34} pwsMB={5:n1}' -f $w.ProcessId, $w.ParentProcessId, $w.Name, $type, $svc, ($pmap[[int]$w.ProcessId] / 1MB)
}
$j = Get-Content -Raw $out | ConvertFrom-Json
$r = $j.flyoutRectPx; $m = 24; $X = [int]$r.x - $m; $Y = [int]$r.y - $m; $W = [int]$r.w + 2 * $m; $H = [int]$r.h + 2 * $m
$bmp = New-Object System.Drawing.Bitmap $W, $H; $g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($X, $Y, 0, 0, (New-Object System.Drawing.Size $W, $H))
$bmp.Save("$D\$Tag.png", [System.Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $bmp.Dispose()
"shot $D\$Tag.png"
if (-not $p.WaitForExit(30000)) { Stop-Process -Id $p.Id -Force }
"exit=$($p.ExitCode)"
Start-Sleep -Seconds 4
foreach ($k in $tree.Keys) { if (Get-Process -Id $k -ErrorAction SilentlyContinue) { "leftover $k"; Stop-Process -Id $k -Force } }
