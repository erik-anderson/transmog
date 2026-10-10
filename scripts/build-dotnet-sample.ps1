# Convenience entry point; implementation is portable Python.
[CmdletBinding()]
param([ValidateSet('Debug','Release')][string]$Configuration = 'Debug')
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @('--configuration', $Configuration)
Invoke-TransmogPython 'build-dotnet-sample.py' $arguments
