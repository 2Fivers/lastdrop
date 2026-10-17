<#
.SYNOPSIS
Renders the README's screenshots from the built-in demo data: the flyout, the hover card and the
usage history window, light and dark, into docs\assets\screenshots. With -Check, tells whether
the committed ones are up to date instead.

.DESCRIPTION
Builds the app and runs `lastdrop.exe --demo --preview-*` (crates\app\src\preview.rs) once per
colour mode. The render is headless: no window opens, and --demo uses invented data only, so
nothing of this machine's accounts, history or settings ends up in the pictures. The demo's clock
is fixed, so the pictures change only when the UI, the demo or the fonts do.

The dev build is used: it renders the same pixels as the release build and builds much faster.
The pictures are rendered at scale 2 so they stay sharp on high-DPI screens; the README shows
them at their logical size (half their pixels). They have no rounded corners, border or shadow:
DWM draws those around the real windows.

oxipng compresses them without loss. Its version is pinned so every machine writes the same
bytes; the first run installs it into target\tools with `cargo install`.

-Check renders into -OutDir (by default a temporary folder) and compares the pixels with the
committed pictures without changing them. It runs unattended: exit code 0 when every picture
matches, 1 when one is stale or missing, or when anything fails. -OutDir keeps the fresh
pictures, ready to copy into docs\assets\screenshots.

.EXAMPLE
pwsh tools\screenshots\screenshots.ps1

.EXAMPLE
pwsh tools\screenshots\screenshots.ps1 -Check -OutDir target\screenshots
#>
[CmdletBinding()]
param(
    [switch] $Check,
    [string] $OutDir
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$committed = Join-Path $root "docs\assets\screenshots"
$exe = Join-Path $root "target\debug\lastdrop.exe"
$scale = 2
$oxipngVersion = "10.2.1"
$tools = Join-Path $root "target\tools"
$oxipng = Join-Path $tools "bin\oxipng.exe"
$names = foreach ($mode in "light", "dark") { foreach ($view in "flyout", "hovercard", "history") { "$view-$mode.png" } }

# The size and a hash of the decoded pixels of a PNG, so a re-compressed file still matches.
function Pixels([string] $path) {
    $bitmap = [System.Drawing.Bitmap]::new($path)
    try {
        $rect = [System.Drawing.Rectangle]::new(0, 0, $bitmap.Width, $bitmap.Height)
        $data = $bitmap.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly,
            [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        try {
            $bytes = [byte[]]::new($data.Stride * $data.Height)
            [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $bytes, 0, $bytes.Length)
        } finally {
            $bitmap.UnlockBits($data)
        }
        "$($bitmap.Width)x$($bitmap.Height) $([Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($bytes)))"
    } finally {
        $bitmap.Dispose()
    }
}

if ($Check) {
    Add-Type -AssemblyName System.Drawing
    $temporary = -not $OutDir
    $out = if ($temporary) {
        Join-Path ([System.IO.Path]::GetTempPath()) "lastdrop-screenshots-$([guid]::NewGuid().ToString('N'))"
    } else {
        $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutDir)
    }
} else {
    $temporary = $false
    $out = $committed
}

Push-Location $root
try {
    cargo build -p lastdrop --locked
    if ($LASTEXITCODE) { throw "cargo build" }
    if (-not (Test-Path $oxipng) -or (& $oxipng --version) -ne "oxipng $oxipngVersion") {
        cargo install oxipng --version $oxipngVersion --locked --root $tools
        if ($LASTEXITCODE) { throw "cargo install oxipng" }
    }
    New-Item -ItemType Directory -Force $out | Out-Null

    foreach ($mode in "light", "dark") {
        $files = [ordered]@{
            "--preview-flyout"    = Join-Path $out "flyout-$mode.png"
            "--preview-hovercard" = Join-Path $out "hovercard-$mode.png"
            "--preview-history"   = Join-Path $out "history-$mode.png"
        }
        # --demo first and always: without it the previews read this machine's accounts.
        $arguments = @("--demo", "--scale", "$scale")
        if ($mode -eq "dark") { $arguments += "--dark" }
        foreach ($flag in $files.Keys) {
            Remove-Item -ErrorAction SilentlyContinue $files[$flag], "$($files[$flag]).error.txt"
            $arguments += $flag, "`"$($files[$flag])`""
        }
        # lastdrop.exe is a GUI program: Start-Process -Wait, or the shell does not wait for it.
        $p = Start-Process -FilePath $exe -ArgumentList $arguments -Wait -PassThru -NoNewWindow
        foreach ($file in $files.Values) {
            # The preview leaves its reason next to the file it could not write.
            if (Test-Path "$file.error.txt") { throw "$($file): $(Get-Content -Raw "$file.error.txt")" }
            if (-not (Test-Path $file)) { throw "$file was not written (exit code $($p.ExitCode))" }
        }
        if ($p.ExitCode) { throw "lastdrop.exe exited with $($p.ExitCode)" }
    }

    & $oxipng --opt max --strip safe --quiet ($names | ForEach-Object { Join-Path $out $_ })
    if ($LASTEXITCODE) { throw "oxipng" }

    if (-not $Check) {
        foreach ($name in $names) { "{0,-22} {1,5:N0} KB" -f $name, ((Get-Item (Join-Path $out $name)).Length / 1KB) }
        exit 0
    }
    $stale = @($names | Where-Object {
        $old = Join-Path $committed $_
        -not (Test-Path $old) -or (Pixels $old) -ne (Pixels (Join-Path $out $_))
    })
    if ($stale.Count) {
        Write-Host "::error::README screenshots are out of date: $($stale -join ', '). Run pwsh tools\screenshots\screenshots.ps1 and commit docs\assets\screenshots."
        exit 1
    }
    Write-Host "README screenshots are up to date."
    exit 0
} finally {
    Pop-Location
    if ($temporary -and (Test-Path $out)) { Remove-Item -Recurse -Force $out }
}
