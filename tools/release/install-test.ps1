<#
.SYNOPSIS
Installs and uninstalls a Lastdrop-Setup.exe the way winget does and checks what each step
leaves behind (updates-spec §2, packaging/winget). For a clean Windows user: the release
workflow runs it on a fresh runner. On your own account it would replace your installed copy.

.DESCRIPTION
1. `Lastdrop-Setup.exe --install --quiet` exits 0; the program, the Start menu shortcut and the
   "Installed apps" entry (DisplayVersion = -Version, publisher, QuietUninstallString) exist, and
   the installed app is running.
2. The entry's QuietUninstallString exits 0; the entry and the shortcut are gone, the app has
   quit, and the install folder is removed shortly after.

.EXAMPLE
pwsh tools\release\install-test.ps1 -Setup dist\Lastdrop-Setup.exe -Version 0.1.0
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Setup,
    [Parameter(Mandatory)] [string] $Version
)

$ErrorActionPreference = "Stop"
$dir = Join-Path $env:LOCALAPPDATA "Programs\Lastdrop"
$exe = Join-Path $dir "lastdrop.exe"
$key = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Lastdrop"
$shortcut = Join-Path ([Environment]::GetFolderPath("Programs")) "Lastdrop.lnk"
$failures = [System.Collections.Generic.List[string]]::new()

function Check([bool] $ok, [string] $what) {
    if ($ok) { Write-Host "ok   $what" } else { Write-Host "FAIL $what"; $failures.Add($what) }
}

# Runs a program and waits for it (at most 3 minutes); returns its exit code.
function Run([string] $file, [string[]] $arguments) {
    $p = Start-Process -FilePath $file -ArgumentList $arguments -PassThru -WindowStyle Hidden
    if (-not $p.WaitForExit(180000)) {
        $p.Kill()
        throw "$file $arguments did not finish within 3 minutes"
    }
    $p.ExitCode
}

function Installed-App {
    @(Get-Process -Name lastdrop -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe })
}

if (Test-Path $key) { throw "Lastdrop is installed already: run this for a clean Windows user" }

# 1. Install.
$code = Run (Resolve-Path $Setup) @("--install", "--quiet")
Check ($code -eq 0) "--install --quiet exits 0 (got $code)"
Check (Test-Path $exe) "the program is $exe"
Check (Test-Path $shortcut) "the Start menu shortcut is $shortcut"
$entry = Get-ItemProperty $key -ErrorAction SilentlyContinue
Check ($null -ne $entry) "the Installed apps entry exists"
$quiet = $null
if ($entry) {
    Check ($entry.DisplayVersion -eq $Version) "DisplayVersion is $Version (got $($entry.DisplayVersion))"
    Check ($entry.Publisher -eq "Adinosaur AB") "Publisher is Adinosaur AB (got $($entry.Publisher))"
    $quiet = $entry.QuietUninstallString
    Check ($quiet -eq "`"$exe`" --uninstall --quiet") "QuietUninstallString runs the program with --uninstall --quiet (got $quiet)"
}
$running = $false
for ($i = 0; $i -lt 20 -and -not $running; $i++) {
    Start-Sleep -Milliseconds 500
    $running = (Installed-App).Count -gt 0
}
Check $running "the installed app is running"

# 2. Uninstall, as winget does: the entry's QuietUninstallString through cmd.
if ($quiet) {
    $code = Run "$env:SystemRoot\System32\cmd.exe" @("/d", "/s", "/c", "`"$quiet`"")
    Check ($code -eq 0) "the QuietUninstallString exits 0 (got $code)"
    Check (-not (Test-Path $key)) "the Installed apps entry is gone"
    Check (-not (Test-Path $shortcut)) "the Start menu shortcut is gone"
    Check ((Installed-App).Count -eq 0) "the app has quit"
    # A hidden cmd.exe removes the folder two seconds after the uninstaller ends.
    $gone = $false
    for ($i = 0; $i -lt 30 -and -not $gone; $i++) {
        Start-Sleep -Milliseconds 500
        $gone = -not (Test-Path $dir)
    }
    Check $gone "the install folder is removed"
}

if ($failures.Count) {
    Write-Host "::error::install test: $($failures.Count) check(s) failed"
    exit 1
}
Write-Host "install test: every check passed"
