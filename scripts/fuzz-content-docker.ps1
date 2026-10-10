# Convenience entry point; implementation is portable Python.
[CmdletBinding()]
param([ValidateRange(1,1000000000)][int]$Runs = 10000, [string]$Image = 'transmog-content-fuzz:nightly-2026-10-01')
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @('--runs', $Runs, '--image', $Image)
Invoke-TransmogPython 'fuzz-content-docker.py' $arguments
