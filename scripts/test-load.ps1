# Convenience entry point; implementation is portable Python.
[CmdletBinding()]
param([ValidateSet('both','cli','desktop')][string]$Target = 'both', [ValidateSet('disk','memory')][string]$Storage = 'disk', [string]$Python, [switch]$Smoke, [switch]$Headed, [switch]$SkipBuild, [switch]$SkipDependencyInstall, [switch]$SkipBrowserInstall, [ValidateRange(1,10000)][int]$Navigations = 100, [ValidateRange(1,10000)][int]$RequestsPerNavigation = 100, [ValidateRange(1,32)][int]$Concurrency = 8, [ValidateRange(1,240)][int]$TimeoutMinutes = 30, [string]$Output = 'artifacts/load')
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @('--target', $Target, '--storage', $Storage, '--concurrency', $Concurrency, '--timeout-minutes', $TimeoutMinutes, '--output', $Output)
if ($Python) { $arguments += @('--python', $Python) }
foreach ($flag in @('Smoke','Headed','SkipBuild','SkipDependencyInstall','SkipBrowserInstall')) {
    if (Get-Variable -Name $flag -ValueOnly) { $arguments += '--' + ([regex]::Replace($flag, '(?<!^)([A-Z])', '-$1')).ToLowerInvariant() }
}
if ($PSBoundParameters.ContainsKey('Navigations')) { $arguments += @('--navigations', $Navigations) }
if ($PSBoundParameters.ContainsKey('RequestsPerNavigation')) { $arguments += @('--requests-per-navigation', $RequestsPerNavigation) }
Invoke-TransmogPython 'test-load.py' $arguments
