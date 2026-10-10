# Convenience entry point for the independent native ZIP compatibility check.
[CmdletBinding()]
param([string]$SevenZipPath)
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @()
if ($SevenZipPath) { $arguments += @('--seven-zip-path', $SevenZipPath) }
Invoke-TransmogPython 'test-saz-interop.py' $arguments
