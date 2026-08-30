[CmdletBinding()]
param([Parameter(Mandatory)][string]$ReleaseTag, [Parameter(Mandatory)][string]$BinaryPath)
$ErrorActionPreference = 'Stop'
$expected = 'modelvault ' + ($ReleaseTag -replace '^v', '')
$reported = (& $BinaryPath -V).Trim()
if ($reported -ne $expected) { throw "Release binary reports '$reported'; expected '$expected'." }
