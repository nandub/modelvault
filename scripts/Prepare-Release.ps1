[CmdletBinding(SupportsShouldProcess)]
param(
    [Parameter(Mandatory)] [ValidatePattern('^\d+\.\d+\.\d+$')] [string]$Version,
    [string]$ReleaseTitle = 'Release'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

function Write-ReleaseFile([string]$Path, [string]$Content) {
    if ($PSCmdlet.ShouldProcess($Path, "set release version to $Version")) {
        [System.IO.File]::WriteAllText($Path, $Content, [System.Text.UTF8Encoding]::new($false))
    }
}

$tomlPath = Join-Path $root 'Cargo.toml'
$lockPath = Join-Path $root 'Cargo.lock'
$changelogPath = Join-Path $root 'CHANGELOG.md'
$toml = [System.IO.File]::ReadAllText($tomlPath)
$newToml = [regex]::Replace($toml, '(?ms)(^\[package\]\s.*?^version\s*=\s*")[^"]+(")', "`${1}$Version`${2}", 1)
if ($newToml -eq $toml) { throw 'Could not update Cargo.toml.' }
$lock = [System.IO.File]::ReadAllText($lockPath)
$newLock = [regex]::Replace($lock, '(?m)(^name = "modelvault"\r?\nversion\s*=\s*")[^"]+(")', "`${1}$Version`${2}", 1)
if ($newLock -eq $lock) { throw 'Could not update the ModelVault entry in Cargo.lock.' }
$changelog = [System.IO.File]::ReadAllText($changelogPath)
$unreleased = [regex]::Match($changelog, '(?ms)^## Unreleased\r?\n(?<notes>.*?)(?=^## |\z)')
if (-not $unreleased.Success -or [string]::IsNullOrWhiteSpace($unreleased.Groups['notes'].Value)) {
    throw 'CHANGELOG.md needs a non-empty Unreleased section.'
}
$newline = if ($changelog.Contains("`r`n")) { "`r`n" } else { "`n" }
$newChangelog = $changelog.Remove($unreleased.Index, $unreleased.Length).Insert($unreleased.Index, "## $Version - $ReleaseTitle$newline" + $unreleased.Groups['notes'].Value)
Write-ReleaseFile $tomlPath $newToml
Write-ReleaseFile $lockPath $newLock
Write-ReleaseFile $changelogPath $newChangelog
Write-Host "Prepared ModelVault $Version. Validate, commit, tag v$Version, and push."
