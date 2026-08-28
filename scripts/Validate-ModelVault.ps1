[CmdletBinding()]
param(
    [switch]$WithS3,
    [switch]$WithMinio,
    [switch]$SecurityTools
)

$ErrorActionPreference = 'Stop'

function Get-ModelVaultPackageVersion {
    param([string]$CargoTomlPath)

    $versionLine = Get-Content -LiteralPath $CargoTomlPath |
        Where-Object { $_ -match '^version\s*=\s*"([^"]+)"\s*$' } |
        Select-Object -First 1

    if (-not $versionLine -or $versionLine -notmatch '^version\s*=\s*"([^"]+)"\s*$') {
        throw 'Unable to determine ModelVault package version from Cargo.toml.'
    }

    return $Matches[1]
}

function Test-CargoLockMatchesPackageVersion {
    param(
        [string]$CargoLockPath,
        [string]$ExpectedVersion
    )

    if (-not (Test-Path -LiteralPath $CargoLockPath)) {
        return $false
    }

    $lines = Get-Content -LiteralPath $CargoLockPath
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -eq 'name = "modelvault"') {
            for ($j = $i + 1; $j -lt [Math]::Min($i + 6, $lines.Count); $j++) {
                if ($lines[$j] -match '^version = "([^"]+)"$') {
                    return ($Matches[1] -eq $ExpectedVersion)
                }
            }
        }
    }

    return $false
}

$repo = Split-Path -Parent $PSScriptRoot
Push-Location $repo
try {
    if ($WithMinio -and -not $WithS3) {
        throw '-WithMinio requires -WithS3 because the MinIO acceptance test uses the optional S3 feature.'
    }

    $packageVersion = Get-ModelVaultPackageVersion -CargoTomlPath '.\Cargo.toml'
    $lockMatches = Test-CargoLockMatchesPackageVersion -CargoLockPath '.\Cargo.lock' -ExpectedVersion $packageVersion

    if (-not $lockMatches) {
        Write-Host "Cargo.lock is missing or does not match ModelVault $packageVersion; refreshing it before locked validation."
        cargo generate-lockfile
        if ($LASTEXITCODE -ne 0) { throw 'cargo generate-lockfile failed' }
    }

    cargo build --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    cargo test --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }

    cargo clippy --locked --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'cargo clippy failed' }

    if ($WithS3) {
        cargo build --locked --features s3
        if ($LASTEXITCODE -ne 0) { throw 'cargo build --features s3 failed' }
    }

    if ($WithMinio) {
        Write-Host 'Running disposable MinIO S3 acceptance test.'
        & .\scripts\Test-S3-MinIO.ps1
    }

    if ($SecurityTools) {
        if (Get-Command cargo-audit -ErrorAction SilentlyContinue) {
            cargo audit
            if ($LASTEXITCODE -ne 0) { throw 'cargo audit failed' }
        }
        else {
            Write-Warning 'cargo-audit is not installed; skipping cargo audit.'
        }

        if (Get-Command cargo-deny -ErrorAction SilentlyContinue) {
            cargo deny check
            if ($LASTEXITCODE -ne 0) { throw 'cargo deny check failed' }
        }
        else {
            Write-Warning 'cargo-deny is not installed; skipping cargo deny check.'
        }
    }
}
finally {
    Pop-Location
}
