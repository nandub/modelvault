[CmdletBinding()]
param([Parameter(Mandatory)][string]$ReleaseTag, [Parameter(Mandatory)][string]$Repository, [string]$WorkingDirectory = '.')
$ErrorActionPreference = 'Stop'
$asset = "modelvault-$ReleaseTag-x86_64-unknown-linux-gnu.zip"
$download = Join-Path $WorkingDirectory 'release-smoke-download'
$extract = Join-Path $WorkingDirectory 'release-smoke-extract'
New-Item -ItemType Directory -Force -Path $download | Out-Null
gh release download $ReleaseTag --repo $Repository --pattern $asset --pattern SHA256SUMS --dir $download
if ($LASTEXITCODE -ne 0) { throw 'Unable to download published smoke-test assets.' }
$line = Get-Content (Join-Path $download SHA256SUMS) | Where-Object { $_ -match "  $([regex]::Escape($asset))$" }
if (@($line).Count -ne 1) { throw "Expected one checksum entry for $asset." }
$expected = ($line -split '\s+', 2)[0].ToLowerInvariant()
$actual = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $download $asset)).Hash.ToLowerInvariant()
if ($actual -ne $expected) { throw "Checksum mismatch for $asset. Expected $expected; got $actual." }
Expand-Archive -LiteralPath (Join-Path $download $asset) -DestinationPath $extract -Force
& (Join-Path $extract modelvault) -V
if ($LASTEXITCODE -ne 0) { throw 'Published ModelVault binary failed its version check.' }
