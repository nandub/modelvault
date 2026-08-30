[CmdletBinding()]
param([Parameter(Mandatory)][string]$BinaryPath, [Parameter(Mandatory)][string]$ArchivePath)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $ArchivePath) | Out-Null
if ($IsWindows) { Compress-Archive -Path $BinaryPath -DestinationPath $ArchivePath -Force }
else { & zip -j $ArchivePath $BinaryPath; if ($LASTEXITCODE -ne 0) { throw 'Unable to create release archive.' } }
