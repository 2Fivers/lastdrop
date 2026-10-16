# The tray process of `measure.ps1 -Variants lastdrop-app` against the memory budget
# (docs/ARCHITECTURE.md, "Goals that shape everything" 1): its own private working set per phase
# of the bench timeline (crates/app/src/bench.rs), the dialog process beside it. Writes
# results\app-tables.md.
param(
    [string]$ResultsDir = (Join-Path $PSScriptRoot 'results'),
    [string]$Variant = 'lastdrop-app',
    [double]$BudgetMB = 10
)
$ErrorActionPreference = 'Stop'
$windows = [ordered]@{
    'A idle, readout (5-9s)'       = @(5, 9)
    'B flyout cold (12-18s)'       = @(12, 18)
    'C1 hidden (22-28s)'           = @(22, 28)
    'D flyout warm (32-38s)'       = @(32, 38)
    'C2 hidden again (42-48s)'     = @(42, 48)
    'E Settings open (52-58s)'     = @(52, 58)
    'F Settings closed (62-68s)'   = @(62, 68)
}
function Med([double[]]$v) { if (-not $v -or $v.Count -eq 0) { return [double]::NaN }; $s = $v | Sort-Object; $n = $s.Count; if ($n % 2) { $s[[int](($n - 1) / 2)] } else { ($s[$n / 2 - 1] + $s[$n / 2]) / 2 } }
function MB($b) { if ([double]::IsNaN($b)) { 'n/a' } else { '{0:n2}' -f ($b / 1MB) } }

$runs = New-Object System.Collections.Generic.List[object]
foreach ($meta in Get-ChildItem $ResultsDir -Filter "$Variant-run*-meta.json" | Sort-Object { [int]($_.Name -replace '\D', '') }) {
    $tag = $meta.Name -replace '-meta\.json$', ''
    $m = Get-Content -Raw $meta.FullName | ConvertFrom-Json
    $jf = Join-Path $ResultsDir "$tag.json"
    if (-not (Test-Path $jf)) { Write-Warning "$tag has no bench JSON; skipped"; continue }
    $j = Get-Content -Raw $jf | ConvertFrom-Json
    $t0 = [double]$j.t0EpochMs
    $root = [int]$m.pid
    $procs = Import-Csv (Join-Path $ResultsDir "$tag-procs.csv")
    foreach ($p in $procs) { $p | Add-Member tAppS (([double]$p.epochMs - $t0) / 1000.0) }
    $mine = @($procs | Where-Object { [int]$_.pid -eq $root })
    $dialog = @($procs | Where-Object { [int]$_.pid -ne $root -and $_.name -eq 'lastdrop.exe' })
    $o = [ordered]@{ run = $tag -replace '.*-run', ''; exit = $m.exitCode; leftovers = @($m.leftoversKilled).Count
        others = (@($procs | Where-Object { [int]$_.pid -ne $root } | ForEach-Object { $_.name } | Sort-Object -Unique) -join ' ') }
    foreach ($wn in $windows.Keys) {
        $lo, $hi = $windows[$wn]
        $s = @($mine | Where-Object { $_.tAppS -ge $lo -and $_.tAppS -le $hi })
        $o["$wn|pws"] = Med ($s | ForEach-Object { [double]$_.privateWS_perf })
        $o["$wn|pb"] = Med ($s | ForEach-Object { [double]$_.privateBytes })
        $o["$wn|ws"] = Med ($s | ForEach-Object { [double]$_.workingSet })
        $o["$wn|thr"] = Med ($s | ForEach-Object { [double]$_.threads })
        $o["$wn|n"] = $s.Count
        $d = @($dialog | Where-Object { $_.tAppS -ge $lo -and $_.tAppS -le $hi })
        $o["$wn|dialog"] = Med ($d | ForEach-Object { [double]$_.privateWS_perf })
    }
    $o['peak'] = ($mine | ForEach-Object { [double]$_.privateWS_perf } | Measure-Object -Maximum).Maximum
    $runs.Add([pscustomobject]$o)
}
if ($runs.Count -eq 0) { throw "no $Variant runs in $ResultsDir" }

function Agg([string]$key) {
    $vals = @($runs | ForEach-Object { [double]$_.$key } | Where-Object { -not [double]::IsNaN($_) })
    if ($vals.Count -eq 0) { return 'n/a' }
    '{0} ({1}–{2})' -f (MB (Med $vals)), (MB ($vals | Measure-Object -Minimum).Minimum), (MB ($vals | Measure-Object -Maximum).Maximum)
}
$sb = New-Object System.Text.StringBuilder
$w = @($windows.Keys)
[void]$sb.AppendLine("### Tray process, private working set (Task Manager ""Memory""), MB: median of $($runs.Count) runs (min–max)`n")
[void]$sb.AppendLine('| phase | private WS | private bytes | working set | threads | dialog process private WS |')
[void]$sb.AppendLine('|---|---|---|---|---|---|')
foreach ($wn in $w) {
    $thr = Med @($runs | ForEach-Object { [double]$_."$wn|thr" })
    [void]$sb.AppendLine("| $wn | $(Agg "$wn|pws") | $(Agg "$wn|pb") | $(Agg "$wn|ws") | $thr | $(Agg "$wn|dialog") |")
}
[void]$sb.AppendLine("| peak (whole run) | $(Agg 'peak') | | | | |")
[void]$sb.AppendLine()
[void]$sb.AppendLine("### Per run: tray process private WS, MB (A / B / C1 / D / C2 / E / F)`n")
[void]$sb.AppendLine('| run | ' + (($w | ForEach-Object { ($_ -split ' ')[0] }) -join ' | ') + ' | peak | C2−C1 | F−C2 | samples/phase | exit | other processes |')
[void]$sb.AppendLine('|---|' + (($w | ForEach-Object { '---' }) -join '|') + '|---|---|---|---|---|---|')
foreach ($o in $runs) {
    $cells = $w | ForEach-Object { MB $o."$_|pws" }
    $c1 = $o."$($w[2])|pws"; $c2 = $o."$($w[4])|pws"; $f = $o."$($w[6])|pws"
    $cnts = ($w | ForEach-Object { $o."$_|n" }) -join '/'
    [void]$sb.AppendLine("| $($o.run) | $($cells -join ' | ') | $(MB $o.peak) | $(MB ($c2 - $c1)) | $(MB ($f - $c2)) | $cnts | $($o.exit) | $($o.others) |")
}
[void]$sb.AppendLine()
$budget = $BudgetMB * 1MB
$worst = ($runs | ForEach-Object { $o = $_; $w | ForEach-Object { [double]$o."$_|pws" } } | Where-Object { -not [double]::IsNaN($_) } | Measure-Object -Maximum).Maximum
$verdict = if ($worst -lt $budget) { 'UNDER' } else { 'OVER' }
[void]$sb.AppendLine("Budget $BudgetMB MB: $verdict (highest phase median $(MB $worst) MB, highest sample $(MB ($runs | Measure-Object peak -Maximum).Maximum) MB).")
$sb.ToString() | Set-Content -Encoding utf8 (Join-Path $ResultsDir 'app-tables.md')
$sb.ToString()
