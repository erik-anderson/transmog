# Convenience entry point; implementation is portable Python.
[CmdletBinding()]
param([string]$Image = 'transmog-linux-tools:rust-1.97.1-nightly-2026-10-01')
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @('--image', $Image)
Invoke-TransmogPython 'test-linux-docker.py' $arguments
