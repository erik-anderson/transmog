# Convenience entry point; version updates and rollback live in Python.
[CmdletBinding(DefaultParameterSetName = 'Set')]
param(
    [Parameter(Mandatory, ParameterSetName='Set')][string]$Version,
    [Parameter(Mandatory, ParameterSetName='Check')][switch]$Check,
    [Parameter(ParameterSetName='Set')][ValidateSet('Canary','Release')][string]$Channel,
    [Parameter(ParameterSetName='Set')][ValidateSet('Canary','Beta','Stable')][string]$ReleaseType,
    [string]$RepositoryRoot = (Split-Path -Parent $PSScriptRoot)
)
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @('--repository-root', $RepositoryRoot)
if ($Check) { $arguments += '--check' } else { $arguments += @('--version', $Version) }
if ($Channel) { $arguments += @('--channel', $Channel) }
if ($ReleaseType) { $arguments += @('--release-type', $ReleaseType) }
Invoke-TransmogPython 'set-release-version.py' $arguments
