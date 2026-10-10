param(
    [string[]]$Variants = @(),          # empty = all
    [int[]]$Runs = @(1, 2, 3),
    [string]$ResultsDir = (Join-Path $PSScriptRoot 'results'),
    [switch]$RoundRobin
)
$ErrorActionPreference = 'Stop'
$SPIKE = (Resolve-Path (Join-Path $PSScriptRoot '..\..\spikes')).Path
New-Item -ItemType Directory -Force $ResultsDir | Out-Null
$LOG = Join-Path $ResultsDir "measure.log"
function Log([string]$m) { $line = "{0:HH:mm:ss.fff} {1}" -f (Get-Date), $m; Add-Content -Path $LOG -Value $line; Write-Host $line }

if (-not ('LdMeasure' -as [type])) { Add-Type -Path (Join-Path $PSScriptRoot 'LdMeasure.cs') }
Add-Type -AssemblyName System.Drawing
$dpiOk = [LdMeasure]::SetProcessDpiAwarenessContext([IntPtr](-4))
[void][LdMeasure]::SetThreadDpiAwarenessContext([IntPtr](-4))
$aw = [LdMeasure]::GetAwarenessFromDpiAwarenessContext([LdMeasure]::GetThreadDpiAwarenessContext())
Log "DPI: SetProcessDpiAwarenessContext(-4)=$dpiOk, thread awareness=$aw (2 = per-monitor)"

$ALL = [ordered]@{
    'slint-software'      = @{ exe = "$SPIKE\slint-flyout\dist\lastdrop-slint-software.exe"; args = '--bench "{OUT}" --variant slint-keepalive-software'; env = @{ SLINT_BACKEND = 'winit-software' } }
    'slint-skia-software' = @{ exe = "$SPIKE\slint-flyout\dist\lastdrop-slint-skia.exe"; args = '--bench "{OUT}" --variant slint-keepalive-skia-software'; env = @{ SLINT_BACKEND = 'winit-skia-software' } }
    'slint-femtovg-wgpu'  = @{ exe = "$SPIKE\slint-flyout\dist\lastdrop-slint-femtovg-wgpu.exe"; args = '--bench "{OUT}" --variant slint-keepalive-femtovg-wgpu'; env = @{ SLINT_BACKEND = 'winit-femtovg-wgpu'; WGPU_BACKEND = 'dx12' } }
    'slint-femtovg-gl'    = @{ exe = "$SPIKE\slint-flyout\dist\lastdrop-slint-femtovg.exe"; args = '--bench "{OUT}" --variant slint-keepalive-femtovg'; env = @{ SLINT_BACKEND = 'winit-femtovg' } }
    'slint-skia-opengl'   = @{ exe = "$SPIKE\slint-flyout\dist\lastdrop-slint-skia.exe"; args = '--bench "{OUT}" --variant slint-keepalive-skia-opengl'; env = @{ SLINT_BACKEND = 'winit-skia-opengl' } }
    'tauri-keepalive'     = @{ exe = "$SPIKE\tauri-flyout\target\release\lastdrop-tauri.exe"; args = '--bench "{OUT}" --mode keepalive'; env = @{} }
    'tauri-destroy'       = @{ exe = "$SPIKE\tauri-flyout\target\release\lastdrop-tauri.exe"; args = '--bench "{OUT}" --mode destroy'; env = @{} }
    'tauri-keepalive-lowmem' = @{ exe = "$SPIKE\tauri-flyout\target\release\lastdrop-tauri.exe"; args = '--bench "{OUT}" --mode keepalive --lowmem'; env = @{} }
}
if ($Variants.Count -eq 0) { $Variants = @($ALL.Keys) }

$cs = New-CimSession
$cat = New-Object System.Diagnostics.PerformanceCounterCategory 'Process'
[void]$cat.ReadCategory()   # first call is slow (~2 s); warm it up

function NowMs { [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }
function FtToMs([long]$ft) { [long](($ft - 116444736000000000) / 10000) }

function Wait-Quiet {
    $waited = 0
    while ($true) {
        $busy = Get-Process | Where-Object { $_.Name -match '^(cargo|rustc|link|lld-link|cl|build-script-.*|lastdrop-.*)$' }
        if (-not $busy) { break }
        if ($waited % 60 -eq 0) { Log ("waiting for quiet machine: " + (($busy | ForEach-Object { "$($_.Name)#$($_.Id)" }) -join ', ')) }
        Start-Sleep -Seconds 5; $waited += 5
        if ($waited -ge 1800) { throw "machine not quiet after 30 min" }
    }
    # settle
    Start-Sleep -Seconds 3
}

function Get-PerfMap {
    $d = $cat.ReadCategory()
    $map = @{}
    $ids = $d['ID Process']
    foreach ($inst in $ids.Values) {
        $name = $inst.InstanceName
        if ($name -eq '_Total' -or $name -eq 'Idle') { continue }
        $procId = [int]$inst.RawValue
        $map[$procId] = [pscustomobject]@{
            inst = $name
            pws  = $d['Working Set - Private'][$name].RawValue
            ws   = $d['Working Set'][$name].RawValue
            pb   = $d['Private Bytes'][$name].RawValue
            cpid = [int]$d['Creating Process ID'][$name].RawValue
        }
    }
    $map
}

function Save-Shot($rect, [string]$path) {
    $m = 24
    $x = [int]$rect.x - $m; $y = [int]$rect.y - $m; $w = [int]$rect.w + 2 * $m; $h = [int]$rect.h + 2 * $m
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($x, $y, 0, 0, (New-Object System.Drawing.Size $w, $h))
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    "$x,$y,$w,$h"
}

function Read-JsonRetry([string]$path) {
    for ($i = 0; $i -lt 20; $i++) {
        try { if (Test-Path $path) { return (Get-Content -Raw $path | ConvertFrom-Json) } } catch { }
        Start-Sleep -Milliseconds 100
    }
    $null
}

function Invoke-Run([string]$variant, [int]$run) {
    $cfg = $ALL[$variant]
    $tag = "$variant-run$run"
    $out = Join-Path $ResultsDir "$tag.json"
    Remove-Item -ErrorAction SilentlyContinue $out, "$out.tmp"
    Wait-Quiet
    [LdMeasure]::Reset()
    $argStr = $cfg.args.Replace('{OUT}', $out)
    foreach ($k in $cfg.env.Keys) { Set-Item "Env:$k" $cfg.env[$k] }
    try {
        $p = Start-Process -FilePath $cfg.exe -ArgumentList $argStr -PassThru
    } finally {
        foreach ($k in $cfg.env.Keys) { Remove-Item "Env:$k" -ErrorAction SilentlyContinue }
    }
    $root = $p.Id
    [void][LdMeasure]::Track($root)
    $q0 = [LdMeasure]::Query($root)
    $t0 = FtToMs $q0[5]
    $rootCreate = $q0[5]
    Log "START $tag pid=$root t0=$t0 exe=$(Split-Path $cfg.exe -Leaf) args=$argStr env=$(($cfg.env.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ' ')"

    $known = @{ $root = $true }
    $names = @{ $root = (Split-Path $cfg.exe -Leaf) }
    $procRows = New-Object System.Collections.Generic.List[object]
    $totRows = New-Object System.Collections.Generic.List[object]
    $prevSys = [LdMeasure]::SysTimes()
    $shot = $null; $shotRect = $null
    for ($k = 0; $k -le 65; $k++) {
        $target = $t0 + $k * 1000 + 250
        $wait = $target - (NowMs)
        if ($wait -gt 0) { Start-Sleep -Milliseconds $wait }
        $ts = NowMs
        $sys = [LdMeasure]::SysTimes()
        $dIdle = $sys[0] - $prevSys[0]; $dTot = ($sys[1] - $prevSys[1]) + ($sys[2] - $prevSys[2])
        $sysPct = if ($dTot -gt 0) { [math]::Round(100.0 * ($dTot - $dIdle) / $dTot, 1) } else { 0 }
        $prevSys = $sys
        $perf = Get-PerfMap
        $wp = Get-CimInstance -CimSession $cs -Query "SELECT ProcessId,ParentProcessId,Name,ThreadCount,CreationDate FROM Win32_Process"
        $byPid = @{}
        foreach ($w in $wp) { $byPid[[int]$w.ProcessId] = $w }
        # build/extend the tree: a process belongs if its parent belongs and it was created after the root
        $changed = $true
        while ($changed) {
            $changed = $false
            foreach ($w in $wp) {
                $wpid = [int]$w.ProcessId
                if ($known.ContainsKey($wpid)) { continue }
                if ($known.ContainsKey([int]$w.ParentProcessId) -and $w.CreationDate.ToFileTimeUtc() -ge ($rootCreate - 10000000)) {
                    $known[$wpid] = $true; $names[$wpid] = $w.Name; [void][LdMeasure]::Track($wpid); $changed = $true
                    Log "  + child pid=$wpid ($($w.Name)) ppid=$($w.ParentProcessId) at T0+$([math]::Round(($ts - $t0)/1000,1))s"
                }
            }
        }
        $alive = @($known.Keys | Where-Object { $byPid.ContainsKey($_) })
        $netProcs = @{}
        if ($alive.Count -gt 0) { foreach ($np in (Get-Process -Id $alive -ErrorAction SilentlyContinue)) { $netProcs[$np.Id] = $np } }
        $sum = [ordered]@{ pb = 0L; ws = 0L; pwsPerf = 0L; pwsPsapi = 0L; puPsapi = 0L; thr = 0; cpu = 0L; n = 0 }
        foreach ($a in $alive) {
            $qq = [LdMeasure]::Query($a)
            if ($null -eq $qq -or $qq[4] -eq 1) { continue }
            $np = $netProcs[$a]; $pf = $perf[$a]; $w = $byPid[$a]
            $row = [pscustomobject]@{
                variant = $variant; run = $run; k = $k; epochMs = $ts; tRelS = [math]::Round(($ts - $t0) / 1000.0, 3)
                pid = $a; ppid = [int]$w.ParentProcessId; name = $w.Name
                privateBytes = if ($np) { $np.PrivateMemorySize64 } else { $qq[0] }
                workingSet = if ($np) { $np.WorkingSet64 } else { $qq[1] }
                privateWS_perf = if ($pf) { [long]$pf.pws } else { -1 }
                privateWS_psapi = $qq[2]
                privateUsage_psapi = $qq[0]
                threads = [int]$w.ThreadCount
                cpuMs = [math]::Round($qq[3] / 10000.0, 1)
            }
            $procRows.Add($row)
            $sum.pb += $row.privateBytes; $sum.ws += $row.workingSet
            $sum.pwsPerf += [math]::Max(0, $row.privateWS_perf); $sum.pwsPsapi += $row.privateWS_psapi; $sum.puPsapi += $row.privateUsage_psapi
            $sum.thr += $row.threads; $sum.cpu += $qq[3]; $sum.n++
        }
        $dur = (NowMs) - $ts
        $totRows.Add([pscustomobject]@{
                variant = $variant; run = $run; k = $k; epochMs = $ts; tRelS = [math]::Round(($ts - $t0) / 1000.0, 3)
                processes = $sum.n; threads = $sum.thr
                privateBytes = $sum.pb; workingSet = $sum.ws; privateWS_perf = $sum.pwsPerf; privateWS_psapi = $sum.pwsPsapi
                cpuMsAlive = [math]::Round($sum.cpu / 10000.0, 1); systemCpuPct = $sysPct; sampleMs = $dur
            })
        if ($k -eq 14) {
            $j = Read-JsonRetry $out
            if ($j -and $j.flyoutRectPx) {
                $png = Join-Path $ResultsDir "$tag.png"
                $shotRect = Save-Shot $j.flyoutRectPx $png
                $shot = $png
                Log "  shot $png rect(+24)=$shotRect at T0+$([math]::Round(((NowMs) - $t0)/1000,1))s"
            } else { Log "  WARN no flyoutRectPx in json at T0+14" }
        }
        $rootQ = [LdMeasure]::Query($root)
        if ($rootQ[4] -eq 1 -and $sum.n -eq 0) { break }
    }
    # make sure the tree is gone
    if (-not $p.WaitForExit(15000)) { Log "  WARN root still running, killing pid $root"; Stop-Process -Id $root -Force -ErrorAction SilentlyContinue; $p.WaitForExit(5000) | Out-Null }
    $leftKilled = @()
    $deadline = (NowMs) + 10000
    while ($true) {
        $still = @($known.Keys | Where-Object { $q = [LdMeasure]::Query($_); $q -and $q[4] -eq 0 })
        if ($still.Count -eq 0) { break }
        if ((NowMs) -gt $deadline) {
            foreach ($s in $still) { Log "  KILL leftover pid=$s ($($names[$s]))"; Stop-Process -Id $s -Force -ErrorAction SilentlyContinue; $leftKilled += $s }
            Start-Sleep -Seconds 1; break
        }
        Start-Sleep -Milliseconds 500
    }
    $cpuTotal = 0L; $cpuRoot = 0L
    foreach ($kp in $known.Keys) { $q = [LdMeasure]::Query($kp); if ($q -and $q[3] -ge 0) { $cpuTotal += $q[3]; if ($kp -eq $root) { $cpuRoot = $q[3] } } }
    $exitCode = try { $p.ExitCode } catch { $null }
    $procRows | Export-Csv -NoTypeInformation -Path (Join-Path $ResultsDir "$tag-procs.csv")
    $totRows | Export-Csv -NoTypeInformation -Path (Join-Path $ResultsDir "$tag-totals.csv")
    $meta = [ordered]@{
        variant = $variant; run = $run; pid = $root; t0CreationEpochMs = $t0; exitCode = $exitCode
        pidsSeen = @($known.Keys | ForEach-Object { "$_ $($names[$_])" })
        cpuTotalMs = [math]::Round($cpuTotal / 10000.0, 1); cpuRootMs = [math]::Round($cpuRoot / 10000.0, 1)
        leftoversKilled = $leftKilled; screenshot = $shot; screenshotRect = $shotRect
        samples = $totRows.Count; maxSampleMs = ($totRows | Measure-Object sampleMs -Maximum).Maximum
    }
    $meta | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $ResultsDir "$tag-meta.json")
    Log "END $tag exit=$exitCode cpuTotal=$($meta.cpuTotalMs)ms pids=$($known.Count) leftovers=$($leftKilled.Count) maxSampleMs=$($meta.maxSampleMs)"
    [LdMeasure]::Reset()
}

$order = @()
if ($RoundRobin) { foreach ($r in $Runs) { foreach ($v in $Variants) { $order += , @($v, $r) } } }
else { foreach ($v in $Variants) { foreach ($r in $Runs) { $order += , @($v, $r) } } }
Log "PLAN: $(( $order | ForEach-Object { "$($_[0])#$($_[1])" }) -join ', ')"
foreach ($o in $order) {
    try { Invoke-Run $o[0] $o[1] } catch { Log "ERROR $($o[0]) run $($o[1]): $_ $($_.ScriptStackTrace)" }
}
Remove-CimSession $cs
Log "ALL DONE"
