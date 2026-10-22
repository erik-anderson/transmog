# Convenience entry point; implementation is portable Python.
[CmdletBinding()]
param(
    [ValidateSet('Debug','Release')][string]$Configuration = 'Debug',
    [switch]$Test
)
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @('--configuration', $Configuration)
if ($Test) { $arguments += '--test' }
Invoke-TransmogPython 'build-sdk.py' $arguments
