# Convenience entry point; implementation is portable Python.
[CmdletBinding()]
param([string]$NoticePath, [string]$SbomPath)
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @()
if ($NoticePath) { $arguments += @('--notice-path', $NoticePath) }
if ($SbomPath) { $arguments += @('--sbom-path', $SbomPath) }
Invoke-TransmogPython 'generate-supply-chain-artifacts.py' $arguments
