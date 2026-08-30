[CmdletBinding()]
param([Parameter(Mandatory)][string]$ReleaseTag, [Parameter(Mandatory)][string]$Repository, [Parameter(Mandatory)][string[]]$AssetPaths, [switch]$CreateRelease)
$ErrorActionPreference = 'Stop'
if ($CreateRelease) { gh release view $ReleaseTag --repo $Repository *> $null; if ($LASTEXITCODE -ne 0) { gh release create $ReleaseTag --repo $Repository --generate-notes; if ($LASTEXITCODE -ne 0) { throw 'Unable to create GitHub Release.' } } }
gh release upload $ReleaseTag $AssetPaths --repo $Repository --clobber
if ($LASTEXITCODE -ne 0) { throw 'Unable to upload release assets.' }
