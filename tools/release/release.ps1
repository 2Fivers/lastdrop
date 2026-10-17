<#
.SYNOPSIS
Builds a Lastdrop release and publishes it to the public releases repository
(docs/design/updates-spec.md §4).

.DESCRIPTION
1. Checks that -Version is the workspace version in Cargo.toml.
2. Runs CI's steps (fmt, clippy, tests) unless -SkipChecks, then builds the release binary.
3. Copies it to dist\Lastdrop-Setup.exe (the app installs itself under that name).
4. Signs it when a certificate is configured (below); otherwise warns and goes on unsigned.
5. Writes dist\SHA256SUMS, which the app checks the download against.
6. Creates the GitHub release v<Version> in -Repo with both files: a draft unless -Publish.

Signing (Authenticode, SHA-256, RFC 3161 timestamp) uses signtool.exe from the Windows SDK and
one of these, in this order:
  LASTDROP_SIGN_THUMBPRINT  the SHA-1 thumbprint of a certificate in the user's store (a
                            hardware token's certificate shows up there too)
  LASTDROP_SIGN_PFX         a .pfx file, with its password in LASTDROP_SIGN_PFX_PASSWORD
  LASTDROP_SIGN_TIMESTAMP   the timestamp server (default http://timestamp.digicert.com)

.EXAMPLE
pwsh tools\release\release.ps1 -Version 0.1.0
pwsh tools\release\release.ps1 -Version 0.1.0 -Publish
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Version,
    [string] $Repo = "2Fivers/lastdrop-releases",
    [string] $NotesFile,
    [switch] $Publish,
    [switch] $SkipChecks
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Push-Location $root
try {
    # 1. The version.
    $cargo = Get-Content "Cargo.toml" -Raw
    if ($cargo -notmatch '(?m)^\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"') {
        throw "no workspace version in Cargo.toml"
    }
    if ($Matches[1] -ne $Version) {
        throw "Cargo.toml says $($Matches[1]), not ${Version}: bump the workspace version first"
    }
    if (git status --porcelain) {
        throw "the working tree has changes: commit or stash them first"
    }

    # 2. CI's steps and the release build.
    if (-not $SkipChecks) {
        cargo fmt --all -- --check; if ($LASTEXITCODE) { throw "cargo fmt" }
        cargo clippy --workspace --all-targets --locked -- -D warnings; if ($LASTEXITCODE) { throw "cargo clippy" }
        cargo test --workspace --locked; if ($LASTEXITCODE) { throw "cargo test" }
    }
    cargo build --release -p lastdrop --locked; if ($LASTEXITCODE) { throw "cargo build" }

    # 3. The installer.
    $dist = Join-Path $root "dist"
    if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
    New-Item -ItemType Directory $dist | Out-Null
    $setup = Join-Path $dist "Lastdrop-Setup.exe"
    Copy-Item "target\release\lastdrop.exe" $setup

    # 4. Signing, when configured.
    $thumbprint = $env:LASTDROP_SIGN_THUMBPRINT
    $pfx = $env:LASTDROP_SIGN_PFX
    if ($thumbprint -or $pfx) {
        $signtool = (Get-Command signtool.exe -ErrorAction SilentlyContinue).Source
        if (-not $signtool) {
            $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
                Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
        }
        if (-not $signtool) { throw "signing is configured but signtool.exe was not found (Windows SDK)" }
        $timestamp = if ($env:LASTDROP_SIGN_TIMESTAMP) { $env:LASTDROP_SIGN_TIMESTAMP } else { "http://timestamp.digicert.com" }
        $sign = @("sign", "/fd", "SHA256", "/tr", $timestamp, "/td", "SHA256", "/d", "Lastdrop")
        if ($thumbprint) {
            $sign += @("/sha1", $thumbprint)
        } else {
            $sign += @("/f", $pfx, "/p", $env:LASTDROP_SIGN_PFX_PASSWORD)
        }
        & $signtool @sign $setup; if ($LASTEXITCODE) { throw "signtool sign" }
        & $signtool verify /pa $setup; if ($LASTEXITCODE) { throw "signtool verify" }
        Write-Host "Signed."
    } else {
        Write-Warning "No signing certificate configured: Lastdrop-Setup.exe is unsigned (SmartScreen will warn)."
    }

    # 5. Checksums (sha256sum format, LF line ends).
    $hash = (Get-FileHash -Algorithm SHA256 $setup).Hash.ToLowerInvariant()
    $sums = Join-Path $dist "SHA256SUMS"
    [System.IO.File]::WriteAllText($sums, "$hash  Lastdrop-Setup.exe`n")
    Write-Host "SHA-256 $hash"

    # 6. The release.
    $tag = "v$Version"
    $notes = if ($NotesFile) { $NotesFile } else {
        $generated = Join-Path $dist "NOTES.md"
        Set-Content $generated "Lastdrop $Version.`n`nDownload Lastdrop-Setup.exe and run it; it installs for your Windows user without administrator rights. SHA256SUMS lists its checksum."
        $generated
    }
    $ghArgs = @("release", "create", $tag, $setup, $sums, "--repo", $Repo, "--title", "Lastdrop $Version", "--notes-file", $notes)
    if (-not $Publish) { $ghArgs += "--draft" }
    gh @ghArgs; if ($LASTEXITCODE) { throw "gh release create" }
    if ($Publish) {
        Write-Host "Published $tag to ${Repo}: running Lastdrops find it within a day."
    } else {
        Write-Host "Draft $tag created in ${Repo}: publish it on GitHub (or run again with -Publish) when it is ready."
    }
} finally {
    Pop-Location
}
