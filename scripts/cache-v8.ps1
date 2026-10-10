# Convenience entry point for preparing the native Windows build cache.
param([string]$ArchivePath)
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @()
if ($ArchivePath) { $arguments += @('--archive-path', $ArchivePath) }
Invoke-TransmogPython 'cache-v8.py' $arguments
