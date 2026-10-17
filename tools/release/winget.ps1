<#
.SYNOPSIS
Writes the winget manifests for a published Lastdrop release (docs/RELEASING.md §4,
docs/design/updates-spec.md §5). Releases themselves are built in the browser: Actions ▸
release ▸ Run workflow.

.DESCRIPTION
Downloads the published release's Lastdrop-Setup.exe and SHA256SUMS to dist\, checks the
checksum and the build's provenance attestation (`gh attestation verify`), writes the
manifests (packaging\winget, with the version, the SHA-256 and the release date) to
dist\winget and runs `winget validate` on them, ready for `wingetcreate submit dist\winget`.

.EXAMPLE
pwsh tools\release\winget.ps1 -Version 0.1.0
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Version,
    [string] $Repo = "2Fivers/lastdrop"
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$tag = "v$Version"
Push-Location $root
try {
    $json = gh release view $tag --repo $Repo --json isDraft,publishedAt
    if ($LASTEXITCODE) { throw "no release $tag in $Repo" }
    $release = $json | ConvertFrom-Json
    if ($release.isDraft) { throw "$tag is still a draft: publish it first (the manifests point at its download URL)" }

    $dist = Join-Path $root "dist"
    if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
    New-Item -ItemType Directory $dist | Out-Null
    gh release download $tag --repo $Repo --dir $dist --pattern Lastdrop-Setup.exe --pattern SHA256SUMS
    if ($LASTEXITCODE) { throw "gh release download" }
    $setup = Join-Path $dist "Lastdrop-Setup.exe"
    $hash = (Get-FileHash -Algorithm SHA256 $setup).Hash
    $listed = (Get-Content (Join-Path $dist "SHA256SUMS") | Where-Object { $_ -match '  Lastdrop-Setup\.exe$' }) -replace '\s.*$', ''
    if ($listed -ne $hash.ToLowerInvariant()) { throw "Lastdrop-Setup.exe does not match SHA256SUMS" }
    gh attestation verify $setup --repo $Repo; if ($LASTEXITCODE) { throw "gh attestation verify" }

    $winget = Join-Path $dist "winget"
    New-Item -ItemType Directory $winget | Out-Null
    $date = ([datetime]$release.publishedAt).ToUniversalTime().ToString("yyyy-MM-dd")
    foreach ($template in Get-ChildItem (Join-Path $root "packaging\winget") -Filter "*.yaml") {
        $text = [System.IO.File]::ReadAllText($template.FullName).
            Replace("{{VERSION}}", $Version).Replace("{{SHA256}}", $hash.ToUpperInvariant()).Replace("{{DATE}}", $date)
        [System.IO.File]::WriteAllText((Join-Path $winget $template.Name), $text.Replace("`r`n", "`n"))
    }
    winget validate --manifest $winget; if ($LASTEXITCODE) { throw "winget validate" }
    Write-Host "winget manifests in $winget. Submit them with: wingetcreate submit `"$winget`""
} finally {
    Pop-Location
}
