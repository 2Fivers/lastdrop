<#
.SYNOPSIS
Renders the README's screenshots from the built-in demo data: the flyout, the hover card and the
usage history window, light and dark, into docs\assets\screenshots.

.DESCRIPTION
Builds the release app and runs `lastdrop.exe --demo --preview-*` (crates\app\src\preview.rs)
once per colour mode. The render is headless: no window opens, and --demo uses invented data
only, so nothing of this machine's accounts, history or settings ends up in the pictures.

The pictures are rendered at scale 2 so they stay sharp on high-DPI screens; the README shows
them at their logical size (half their pixels). They have no rounded corners, border or shadow:
DWM draws those around the real windows. oxipng compresses them without loss; install it with
`winget install Shssoichiro.Oxipng` or `cargo install oxipng --locked`, or pass its path.

.EXAMPLE
pwsh tools\screenshots\screenshots.ps1
#>
[CmdletBinding()]
param(
    [string] $Oxipng = "oxipng"
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$out = Join-Path $root "docs\assets\screenshots"
$exe = Join-Path $root "target\release\lastdrop.exe"
$scale = 2

if (-not (Get-Command $Oxipng -ErrorAction SilentlyContinue)) {
    throw "oxipng not found: winget install Shssoichiro.Oxipng (or cargo install oxipng --locked), or pass -Oxipng <path>"
}

Push-Location $root
try {
    cargo build --release -p lastdrop --locked
    if ($LASTEXITCODE) { throw "cargo build" }
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

    & $Oxipng --opt max --strip safe --quiet (Get-ChildItem $out -Filter "*.png").FullName
    if ($LASTEXITCODE) { throw "oxipng" }
    Get-ChildItem $out -Filter "*.png" | ForEach-Object { "{0,-22} {1,5:N0} KB" -f $_.Name, ($_.Length / 1KB) }
} finally {
    Pop-Location
}
