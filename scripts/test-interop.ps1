# Convenience entry point; implementation is portable Python.
[CmdletBinding()]
param([switch]$SkipBrowserInstall, [switch]$SkipDependencyInstall)
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @()
if ($SkipBrowserInstall) { $arguments += '--skip-browser-install' }
if ($SkipDependencyInstall) { $arguments += '--skip-dependency-install' }
Invoke-TransmogPython 'test-interop.py' $arguments
