# Convenience entry point; implementation is portable Python.
[CmdletBinding()]
param([ValidateSet('Debug','Release')][string]$Configuration = 'Debug', [switch]$LockedDependencies)
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @('--configuration', $Configuration)
if ($LockedDependencies) { $arguments += '--locked-dependencies' }
Invoke-TransmogPython 'build-desktop.py' $arguments
