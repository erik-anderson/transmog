# Convenience entry point for preparing the native Windows build cache.
param([string]$ArchivePath, [ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')][string]$Target)
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @()
if ($ArchivePath) { $arguments += @('--archive-path', $ArchivePath) }
if ($Target) { $arguments += @('--target', $Target) }
Invoke-TransmogPython 'cache-v8.py' $arguments
