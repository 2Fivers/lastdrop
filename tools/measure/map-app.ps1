# Where the tray process's private working set goes (MemMap.cs): heaps, DLL images, pixel
# buffers, stacks. Starts `lastdrop.exe --bench` and maps it at the given seconds after start
# (45 = the flyout hidden after its second show), or maps a running process with -ProcessId.
#   .\tools\measure\map-app.ps1                      # bench run, map at T0 + 45 s
#   .\tools\measure\map-app.ps1 -AppArgs '--no-readout'
#   .\tools\measure\map-app.ps1 -ProcessId 1234
# Quit Lastdrop first (as for measure.ps1). Writes results\map-<tag>-t<s>.csv.
param(
    [int[]]$At = @(45),
    [string]$AppExe = (Join-Path $PSScriptRoot '..\..\target\release\lastdrop.exe'),
    [string]$AppArgs = '',
    [int]$ProcessId = 0,
    [string]$Tag = 'app',
    [string]$ResultsDir = (Join-Path $PSScriptRoot 'results'),
    [int]$Top = 30
)
$ErrorActionPreference = 'Stop'
if (-not ('MemMap' -as [type])) { Add-Type -Path (Join-Path $PSScriptRoot 'MemMap.cs') }
New-Item -ItemType Directory -Force $ResultsDir | Out-Null

function Show-Map([int]$procId, [string]$name) {
    $rows = foreach ($l in [MemMap]::Map($procId)) {
        $f = $l -split "`t"
        [pscustomobject]@{ category = $f[0]; holder = $f[1]; privateKB = [long]$f[2]; shareableKB = [long]$f[3]; allocationKB = [long]$f[4] }
    }
    $rows | Sort-Object privateKB -Descending | Export-Csv -NoTypeInformation (Join-Path $ResultsDir "map-$name.csv")
    $perf = (New-Object System.Diagnostics.PerformanceCounterCategory 'Process').ReadCategory()
    $pws = 0
    foreach ($i in $perf['ID Process'].Values) { if ([int]$i.RawValue -eq $procId) { $pws = $perf['Working Set - Private'][$i.InstanceName].RawValue } }
    "== $name (pid $procId): private working set $([math]::Round($pws / 1KB)) KB, mapped $(($rows | Measure-Object privateKB -Sum).Sum) KB"
    $rows | Group-Object category | ForEach-Object { [pscustomobject]@{ category = $_.Name; privateKB = ($_.Group | Measure-Object privateKB -Sum).Sum; holders = $_.Count } } |
        Sort-Object privateKB -Descending | Format-Table -AutoSize | Out-String
    $rows | Where-Object privateKB -gt 0 | Sort-Object privateKB -Descending | Select-Object -First $Top category, holder, privateKB, allocationKB |
        Format-Table -AutoSize | Out-String -Width 200
}

if ($ProcessId -ne 0) { Show-Map $ProcessId $Tag; return }
if (Get-Process -Name lastdrop -ErrorAction SilentlyContinue) { throw 'Lastdrop is running: quit it first' }
$out = Join-Path $ResultsDir "map-$Tag.json"
$p = Start-Process -FilePath $AppExe -ArgumentList "--bench `"$out`" $AppArgs" -PassThru
$t0 = $p.StartTime
foreach ($s in $At) {
    $wait = ($t0.AddSeconds($s) - (Get-Date)).TotalMilliseconds
    if ($wait -gt 0) { Start-Sleep -Milliseconds $wait }
    Show-Map $p.Id "$Tag-t$s"
}
# The bench opens Settings at T0 + 50 s and quits at T0 + 70 s; close the dialog it leaves open.
if (-not $p.WaitForExit(80000)) { Stop-Process -Id $p.Id -Force }
foreach ($d in Get-CimInstance Win32_Process -Filter "ParentProcessId=$($p.Id) AND Name='lastdrop.exe'") {
    $dp = Get-Process -Id ([int]$d.ProcessId) -ErrorAction SilentlyContinue
    if ($dp) { [void]$dp.CloseMainWindow() }
}
