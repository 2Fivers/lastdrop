param(
    [string]$ResultsDir = (Join-Path $PSScriptRoot 'results')
)
$ErrorActionPreference = 'Stop'
$variants = 'slint-software', 'slint-skia-software', 'slint-femtovg-wgpu', 'slint-femtovg-gl', 'slint-skia-opengl', 'tauri-keepalive', 'tauri-keepalive-lowmem', 'tauri-destroy'
$windows = [ordered]@{
    'A idle before 1st show (5-9s)' = @(5, 9)
    'B shown cold (12-18s)'         = @(12, 18)
    'C1 hidden after hide (22-28s)' = @(22, 28)
    'D shown warm (32-38s)'         = @(32, 38)
    'C2 hidden after 2nd hide (42-48s)' = @(42, 48)
}
function Med([double[]]$v) { if (-not $v -or $v.Count -eq 0) { return [double]::NaN }; $s = $v | Sort-Object; $n = $s.Count; if ($n % 2) { $s[[int](($n - 1) / 2)] } else { ($s[$n / 2 - 1] + $s[$n / 2]) / 2 } }
function MB($b) { '{0:n1}' -f ($b / 1MB) }

$per = @{}   # variant -> list of run objects
$allTot = New-Object System.Collections.Generic.List[object]
$allProc = New-Object System.Collections.Generic.List[object]
foreach ($v in $variants) {
    $per[$v] = New-Object System.Collections.Generic.List[object]
    foreach ($r in 1..3) {
        $tag = "$v-run$r"
        $totF = Join-Path $ResultsDir "$tag-totals.csv"
        if (-not (Test-Path $totF)) { continue }
        $tot = Import-Csv $totF
        $procs = Import-Csv (Join-Path $ResultsDir "$tag-procs.csv")
        $meta = Get-Content -Raw (Join-Path $ResultsDir "$tag-meta.json") | ConvertFrom-Json
        $j = Get-Content -Raw (Join-Path $ResultsDir "$tag.json") | ConvertFrom-Json
        $t0 = [double]$j.t0EpochMs
        foreach ($t in $tot) { $t | Add-Member tAppS ([math]::Round(([double]$t.epochMs - $t0) / 1000.0, 3)); $allTot.Add($t) }
        foreach ($p in $procs) { $p | Add-Member tAppS ([math]::Round(([double]$p.epochMs - $t0) / 1000.0, 3)); $allProc.Add($p) }
        $o = [ordered]@{ run = $r; cold = [double]$j.showLatencyMs.cold; warm = [double]$j.showLatencyMs.warm; cpu = [double]$meta.cpuTotalMs; cpuRoot = [double]$meta.cpuRootMs
            exit = $meta.exitCode; rebased = $j.t0Rebased; partial = $j.partial; complete = $j.complete; focusStolen = $j.focusStolen; renderer = $j.renderer; leftovers = @($meta.leftoversKilled).Count; pidsSeen = @($meta.pidsSeen).Count
            maxSampleMs = $meta.maxSampleMs; sysCpu = (Med ($tot | ForEach-Object { [double]$_.systemCpuPct })) }
        foreach ($wn in $windows.Keys) {
            $lo, $hi = $windows[$wn]
            $s = @($tot | Where-Object { $_.tAppS -ge $lo -and $_.tAppS -le $hi })
            $o["$wn|pws"] = Med ($s | ForEach-Object { [double]$_.privateWS_perf })
            $o["$wn|pwsPsapi"] = Med ($s | ForEach-Object { [double]$_.privateWS_psapi })
            $o["$wn|pb"] = Med ($s | ForEach-Object { [double]$_.privateBytes })
            $o["$wn|ws"] = Med ($s | ForEach-Object { [double]$_.workingSet })
            $o["$wn|n"] = Med ($s | ForEach-Object { [double]$_.processes })
            $o["$wn|thr"] = Med ($s | ForEach-Object { [double]$_.threads })
            $o["$wn|nmax"] = ($s | ForEach-Object { [double]$_.processes } | Measure-Object -Maximum).Maximum
            $o["$wn|pwsMax"] = ($s | ForEach-Object { [double]$_.privateWS_perf } | Measure-Object -Maximum).Maximum
            $o["$wn|count"] = $s.Count
        }
        # peak over whole run
        $o['peakPws'] = ($tot | ForEach-Object { [double]$_.privateWS_perf } | Measure-Object -Maximum).Maximum
        $o['peakPb'] = ($tot | ForEach-Object { [double]$_.privateBytes } | Measure-Object -Maximum).Maximum
        $per[$v].Add([pscustomobject]$o)
    }
}
$allTot | Export-Csv -NoTypeInformation (Join-Path $ResultsDir 'all-totals.csv')
$allProc | Export-Csv -NoTypeInformation (Join-Path $ResultsDir 'all-procs.csv')

function Agg($v, $key, [scriptblock]$fmt) {
    $vals = @($per[$v] | ForEach-Object { [double]$_.$key })
    if ($vals.Count -eq 0) { return 'n/a' }
    $m = Med $vals; $mn = ($vals | Measure-Object -Minimum).Minimum; $mx = ($vals | Measure-Object -Maximum).Maximum
    '{0} ({1}–{2})' -f (& $fmt $m), (& $fmt $mn), (& $fmt $mx)
}
$mbf = { param($x) '{0:n1}' -f ($x / 1MB) }
$msf = { param($x) '{0:n1}' -f $x }
$intf = { param($x) '{0:n0}' -f $x }
$sb = New-Object System.Text.StringBuilder
foreach ($metric in @(@('pws', 'Private working set (Task Manager "Memory"), MB, perf counter'), @('pb', 'Private bytes (commit, PrivateMemorySize64), MB'), @('ws', 'Working set (incl. shared pages, WorkingSet64), MB'), @('n', 'Process count'), @('thr', 'Thread count'))) {
    $key, $title = $metric
    [void]$sb.AppendLine("### $title — median of 3 runs (min–max)`n")
    [void]$sb.AppendLine('| variant | ' + (($windows.Keys) -join ' | ') + ' |')
    [void]$sb.AppendLine('|---|' + (($windows.Keys | ForEach-Object { '---' }) -join '|') + '|')
    foreach ($v in $variants) {
        $f = if ($key -in 'n', 'thr') { $intf } else { $mbf }
        $cells = foreach ($wn in $windows.Keys) { Agg $v "$wn|$key" $f }
        [void]$sb.AppendLine("| $v | " + ($cells -join ' | ') + ' |')
    }
    [void]$sb.AppendLine()
}
[void]$sb.AppendLine("### Latency, CPU, run health — median of 3 runs (min–max)`n")
[void]$sb.AppendLine('| variant | show cold ms | show warm ms | CPU total ms (whole tree, 50 s) | CPU exe only ms | peak private WS MB | peak private bytes MB | exit codes | leftovers killed | t0Rebased | system CPU % (median) |')
[void]$sb.AppendLine('|---|---|---|---|---|---|---|---|---|---|---|')
foreach ($v in $variants) {
    $ex = ($per[$v] | ForEach-Object { $_.exit }) -join ','
    $lo = ($per[$v] | Measure-Object leftovers -Sum).Sum
    $rb = ($per[$v] | ForEach-Object { if ($null -eq $_.rebased) { '-' } else { $_.rebased } }) -join ','
    [void]$sb.AppendLine("| $v | $(Agg $v 'cold' $msf) | $(Agg $v 'warm' $msf) | $(Agg $v 'cpu' $intf) | $(Agg $v 'cpuRoot' $intf) | $(Agg $v 'peakPws' $mbf) | $(Agg $v 'peakPb' $mbf) | $ex | $lo | $rb | $(Agg $v 'sysCpu' $msf) |")
}
[void]$sb.AppendLine()
[void]$sb.AppendLine("### Per run (private WS MB, perf counter: A / B / C1 / D / C2; latency cold/warm)`n")
[void]$sb.AppendLine('| variant | run | A | B | C1 | D | C2 | cold | warm | CPU ms | procs B | samples/window | renderer |')
[void]$sb.AppendLine('|---|---|---|---|---|---|---|---|---|---|---|---|---|')
foreach ($v in $variants) {
    foreach ($o in $per[$v]) {
        $w = @($windows.Keys)
        $cnts = ($w | ForEach-Object { $o."$_|count" }) -join '/'
        [void]$sb.AppendLine("| $v | $($o.run) | $(MB $o."$($w[0])|pws") | $(MB $o."$($w[1])|pws") | $(MB $o."$($w[2])|pws") | $(MB $o."$($w[3])|pws") | $(MB $o."$($w[4])|pws") | $($o.cold) | $($o.warm) | $($o.cpu) | $($o."$($w[1])|n") | $cnts | $($o.renderer) |")
    }
}
$sb.ToString() | Set-Content (Join-Path $ResultsDir 'tables.md')
$sb.ToString()
