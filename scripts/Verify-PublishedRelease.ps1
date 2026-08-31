[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidatePattern('^v[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$')]
    [string]$ReleaseTag,

    [string]$Repository = 'nandub/modelvault',

    [ValidateSet(
        'x86_64-pc-windows-msvc',
        'x86_64-unknown-linux-gnu',
        'aarch64-unknown-linux-gnu',
        'x86_64-apple-darwin',
        'aarch64-apple-darwin'
    )]
    [string]$Target = 'x86_64-pc-windows-msvc',

    [switch]$KeepDownloads
)

$ErrorActionPreference = 'Stop'

foreach ($command in 'gh', 'cosign') {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "Required command '$command' was not found on PATH."
    }
}

$archiveName = "modelvault-$ReleaseTag-$Target.zip"
$checksumName = 'SHA256SUMS'
$archiveBundleName = "$archiveName.sigstore.json"
$checksumBundleName = "$checksumName.sigstore.json"
$downloadDirectory = Join-Path ([System.IO.Path]::GetTempPath()) "modelvault-release-$([guid]::NewGuid())"

try {
    New-Item -ItemType Directory -Path $downloadDirectory | Out-Null
    gh release download $ReleaseTag --repo $Repository `
        --pattern $archiveName `
        --pattern $checksumName `
        --pattern $archiveBundleName `
        --pattern $checksumBundleName `
        --dir $downloadDirectory
    if ($LASTEXITCODE -ne 0) { throw "Unable to download verification assets for $ReleaseTag." }

    $archivePath = Join-Path $downloadDirectory $archiveName
    $checksumPath = Join-Path $downloadDirectory $checksumName
    $archiveBundlePath = Join-Path $downloadDirectory $archiveBundleName
    $checksumBundlePath = Join-Path $downloadDirectory $checksumBundleName
    foreach ($path in $archivePath, $checksumPath, $archiveBundlePath, $checksumBundlePath) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Release download did not provide expected asset: $path"
        }
    }

    $checksumLine = Get-Content -LiteralPath $checksumPath |
        Where-Object { $_ -match "  $([regex]::Escape($archiveName))$" }
    if (@($checksumLine).Count -ne 1) {
        throw "Expected exactly one SHA-256 entry for $archiveName."
    }
    $expectedHash = ($checksumLine -split '\s+', 2)[0].ToLowerInvariant()
    $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $archivePath).Hash.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        throw "Checksum mismatch for $archiveName. Expected $expectedHash; got $actualHash."
    }
    Write-Host "SHA-256 verified: $archiveName"

    $identity = '^' + [regex]::Escape("https://github.com/$Repository/.github/workflows/release.yml@refs/")
    foreach ($item in @(
        @{ Asset = $archivePath; Bundle = $archiveBundlePath; Name = $archiveName },
        @{ Asset = $checksumPath; Bundle = $checksumBundlePath; Name = $checksumName }
    )) {
        & cosign verify-blob `
            --bundle $item.Bundle `
            --certificate-identity-regexp $identity `
            --certificate-oidc-issuer https://token.actions.githubusercontent.com `
            $item.Asset
        if ($LASTEXITCODE -ne 0) { throw "Sigstore verification failed for $($item.Name)." }
        Write-Host "Sigstore verified: $($item.Name)"
    }

    Write-Host "Release $ReleaseTag verification succeeded for $Target."
    if ($KeepDownloads) {
        Write-Host "Downloaded assets retained at: $downloadDirectory"
        $downloadDirectory = $null
    }
} finally {
    if ($null -ne $downloadDirectory -and (Test-Path -LiteralPath $downloadDirectory)) {
        Remove-Item -LiteralPath $downloadDirectory -Recurse -Force
    }
}
