<#
.SYNOPSIS
Starts a Lastdrop release, and afterwards writes its winget manifests (docs/RELEASING.md,
docs/design/updates-spec.md §4–5). GitHub's runners build the release, not this machine.

.DESCRIPTION
Without -Winget: checks that -Version is the workspace version in Cargo.toml, that the working
tree is clean, that main is checked out and equal to origin/main, and that the tag v<Version>
does not exist yet; then creates the tag on main's commit and pushes it. The push starts
.github/workflows/release.yml, which builds Lastdrop-Setup.exe, tests the installer, attests the
build and creates a draft release. Only repository admins may push v* tags.

With -Winget, once the release is tested and published: downloads its Lastdrop-Setup.exe and
SHA256SUMS to dist\, checks the checksum and the build's provenance attestation
(`gh attestation verify`), and writes the winget manifests (packaging\winget, with the version,
the SHA-256 and the release date) to dist\winget, ready for `wingetcreate submit dist\winget`.

.EXAMPLE
pwsh tools\release\release.ps1 -Version 0.1.0
pwsh tools\release\release.ps1 -Version 0.1.0 -Winget
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Version,
    [string] $Repo = "2Fivers/lastdrop",
    [switch] $Winget
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$tag = "v$Version"
Push-Location $root
try {
    if (-not $Winget) {
        $cargo = Get-Content "Cargo.toml" -Raw
        if ($cargo -notmatch '(?m)^\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"') {
            throw "no workspace version in Cargo.toml"
        }
        if ($Matches[1] -ne $Version) {
            throw "Cargo.toml says $($Matches[1]), not ${Version}: bump the workspace version first (a pull request)"
        }
        if (git status --porcelain) { throw "the working tree has changes: commit or stash them first" }
        if ((git branch --show-current) -ne "main") { throw "check out main first" }
        git fetch --quiet --tags origin; if ($LASTEXITCODE) { throw "git fetch" }
        if ((git rev-parse HEAD) -ne (git rev-parse origin/main)) { throw "main is not origin/main: pull (or push) first" }
        if (git tag --list $tag) { throw "the tag $tag exists already" }

        git tag $tag; if ($LASTEXITCODE) { throw "git tag" }
        git push origin $tag; if ($LASTEXITCODE) { git tag -d $tag | Out-Null; throw "git push $tag" }
        Write-Host "Pushed $tag. The release workflow builds it and creates a draft release:"
        Write-Host "  https://github.com/$Repo/actions/workflows/release.yml"
        Write-Host "Test the draft (docs/RELEASING.md), publish it, then run this again with -Winget."
        return
    }

    # -Winget: the published release's files.
    $release = gh release view $tag --repo $Repo --json isDraft,publishedAt | ConvertFrom-Json
    if ($LASTEXITCODE) { throw "no release $tag in $Repo" }
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
