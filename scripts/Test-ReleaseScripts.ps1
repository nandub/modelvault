[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$releaseScripts = Get-ChildItem (Join-Path $PSScriptRoot 'release') -Filter '*.ps1'
foreach ($script in $releaseScripts) {
    [void][scriptblock]::Create([System.IO.File]::ReadAllText($script.FullName))
}

Push-Location $root
try {
    $version = (cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json).packages |
        Where-Object name -eq 'modelvault' | Select-Object -ExpandProperty version
    & (Join-Path $PSScriptRoot 'release/Verify-ReleaseVersion.ps1') -ReleaseTag "v$version" -RepositoryRoot $root
    cargo build --locked | Out-Host
    & (Join-Path $PSScriptRoot 'release/Verify-ReleaseBinary.ps1') -ReleaseTag "v$version" -BinaryPath (Join-Path $root 'target/debug/modelvault.exe')
} finally { Pop-Location }

Write-Host 'Release scripts parsed and core local checks passed.'
