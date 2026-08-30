[CmdletBinding()]
param([Parameter(Mandatory)][string]$ReleaseTag, [Parameter(Mandatory)][string]$Repository, [string]$OutputDirectory = 'published-assets')
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
gh release download $ReleaseTag --repo $Repository --pattern '*.zip' --dir $OutputDirectory
if ($LASTEXITCODE -ne 0) { throw 'Unable to download published release archives.' }
$archives = @(Get-ChildItem $OutputDirectory -Filter '*.zip' | Sort-Object Name)
if ($archives.Count -ne 5) { throw "Expected five published release archives; found $($archives.Count)." }
$archives | ForEach-Object { "{0}  {1}" -f (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant(), $_.Name } | Set-Content (Join-Path $OutputDirectory 'SHA256SUMS')
foreach ($asset in @($archives.FullName) + (Join-Path $OutputDirectory 'SHA256SUMS')) { & cosign sign-blob --yes --bundle "$asset.sigstore.json" $asset; if ($LASTEXITCODE -ne 0) { throw "Unable to sign $asset." } }
