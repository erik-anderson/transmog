# Convenience entry point; implementation is portable Python.
[CmdletBinding()]
param()
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @()
Invoke-TransmogPython 'test.py' $arguments
