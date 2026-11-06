# Convenience entry point for preparing the native Windows build cache.
param([switch]$PrepareOnly, [ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')][string]$Target)
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @()
if ($PrepareOnly) { $arguments += '--prepare-only' }
if ($Target) { $arguments += @('--target', $Target) }
Invoke-TransmogPython 'cache-v8.py' $arguments
