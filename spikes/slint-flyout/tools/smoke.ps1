# Smoke test: run the spike in bench mode with a compressed timeline, capture ONLY the flyout's
# own rectangle while it is shown, then make sure the process is gone.
param(
    [Parameter(Mandatory)] [string] $Exe,
    [string] $Backend = "",
    [string] $Tag = "run",
    [double] $Timescale = 0.3,
    [string] $Extra = "",
    [hashtable] $EnvVars = @{}
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$outDir = Join-Path $root "smoke"
New-Item -ItemType Directory -Force $outDir | Out-Null
$json = Join-Path $outDir "$Tag.json"
$png = Join-Path $outDir "$Tag.png"
Remove-Item $json, $png -ErrorAction SilentlyContinue

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Dpi { [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v); }
"@
[Dpi]::SetProcessDpiAwarenessContext([IntPtr]::new(-4)) | Out-Null
Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices;
public static class Win {
  delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  public static IntPtr FindFor(int pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); var sb = new StringBuilder(64); GetWindowText(h, sb, 64);
      if (p == (uint)pid && IsWindowVisible(h) && sb.ToString() == "Lastdrop") { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
}
"@

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $Exe
$psi.Arguments = "--bench `"$json`" --timescale $Timescale $Extra"
$psi.UseShellExecute = $false
if ($Backend) { $psi.Environment["SLINT_BACKEND"] = $Backend } else { $psi.Environment.Remove("SLINT_BACKEND") | Out-Null }
foreach ($k in $EnvVars.Keys) { $psi.Environment[$k] = $EnvVars[$k] }
$p = [System.Diagnostics.Process]::Start($psi)
$procId = $p.Id
"pid=$procId"
try {
    # first partial json is written at 12 s * timescale, the window shows at 10 s * timescale
    $deadline = (Get-Date).AddSeconds(20 * $Timescale + 5)
    while (-not (Test-Path $json) -and (Get-Date) -lt $deadline -and -not $p.HasExited) { Start-Sleep -Milliseconds 100 }
    if (Test-Path $json) {
        Start-Sleep -Milliseconds 300
        $j = Get-Content $json -Raw | ConvertFrom-Json
        $r = $j.flyoutRectPx
        if ($r) {
            $bmp = New-Object System.Drawing.Bitmap ([int]$r.w), ([int]$r.h)
            $g = [System.Drawing.Graphics]::FromImage($bmp)
            $g.CopyFromScreen([int]$r.x, [int]$r.y, 0, 0, $bmp.Size)
            $bmp.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
            $g.Dispose(); $bmp.Dispose()
            "captured $png ($($r.w)x$($r.h) at $($r.x),$($r.y))"
            # Also ask DWM for the window's own content (PrintWindow + PW_RENDERFULLCONTENT): this
            # includes flip-model / overlay-plane content a GDI screen copy can miss. No backdrop.
            $hw = [Win]::FindFor($procId)
            if ($hw -ne [IntPtr]::Zero) {
                $bmp = New-Object System.Drawing.Bitmap ([int]$r.w), ([int]$r.h)
                $g = [System.Drawing.Graphics]::FromImage($bmp)
                $hdc = $g.GetHdc(); $ok = [Win]::PrintWindow($hw, $hdc, 2); $g.ReleaseHdc($hdc)
                $bmp.Save((Join-Path $outDir "$Tag-printwindow.png"), [System.Drawing.Imaging.ImageFormat]::Png)
                $g.Dispose(); $bmp.Dispose()
                "printwindow ok=$ok"
            }
            # second capture later in the same show period (window stays up 10 s * timescale)
            Start-Sleep -Milliseconds ([int](10000 * $Timescale * 0.5))
            $bmp = New-Object System.Drawing.Bitmap ([int]$r.w), ([int]$r.h)
            $g = [System.Drawing.Graphics]::FromImage($bmp)
            $g.CopyFromScreen([int]$r.x, [int]$r.y, 0, 0, $bmp.Size)
            $bmp.Save((Join-Path $outDir "$Tag-late.png"), [System.Drawing.Imaging.ImageFormat]::Png)
            $g.Dispose(); $bmp.Dispose()
        } else { "no rect in json" }
        $t = $j.trayIconRectPx
        if ($t) {
            $m = 4
            $bmp = New-Object System.Drawing.Bitmap ([int]$t.w + 2 * $m), ([int]$t.h + 2 * $m)
            $g = [System.Drawing.Graphics]::FromImage($bmp)
            $g.CopyFromScreen([int]$t.x - $m, [int]$t.y - $m, 0, 0, $bmp.Size)
            $bmp.Save((Join-Path $outDir "$Tag-tray.png"), [System.Drawing.Imaging.ImageFormat]::Png)
            $g.Dispose(); $bmp.Dispose()
        }
    } else { "no json produced" }
    $limit = (Get-Date).AddSeconds(50 * $Timescale + 8)
    while (-not $p.HasExited -and (Get-Date) -lt $limit) { Start-Sleep -Milliseconds 200 }
    if ($p.HasExited) { "exited code=$($p.ExitCode)" } else { "still running -> killing" }
}
finally {
    $still = Get-Process -Id $procId -ErrorAction SilentlyContinue
    if ($still) { Stop-Process -Id $procId -Force; "killed $procId" }
}
if (Test-Path $json) { Get-Content $json -Raw }
