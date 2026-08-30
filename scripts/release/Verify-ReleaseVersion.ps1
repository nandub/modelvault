[CmdletBinding()]
param([Parameter(Mandatory)][string]$ReleaseTag, [string]$RepositoryRoot = (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)))
$ErrorActionPreference = 'Stop'
$expected = $ReleaseTag -replace '^v', ''
Push-Location $RepositoryRoot
try {
    $metadata = cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
    $package = @($metadata.packages | Where-Object { $_.name -eq 'modelvault' })
    if ($package.Count -ne 1 -or $package[0].version -ne $expected) { throw "Cargo package version must be $expected for $ReleaseTag." }
    $lockPattern = 'name\s*=\s*"modelvault"\s+version\s*=\s*"{0}"' -f [regex]::Escape($expected)
    if ((Get-Content Cargo.lock -Raw) -notmatch $lockPattern) { throw "Cargo.lock must record ModelVault version $expected." }
} finally { Pop-Location }
