[CmdletBinding()]
param([switch]$Check, [string]$Target)
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$targetArguments = if ($Target) { @('--target', $Target) } else { @() }
$environmentJson = & (Get-TransmogPython) (Join-Path $PSScriptRoot 'dev-env.py') --emit-env @targetArguments
if ($LASTEXITCODE -ne 0) { throw 'Development environment configuration failed.' }
$environment = ($environmentJson -join "`n") | ConvertFrom-Json -AsHashtable
foreach ($entry in $environment.GetEnumerator()) {
    [Environment]::SetEnvironmentVariable($entry.Key, [string]$entry.Value, 'Process')
}
if ($Check) {
    Invoke-TransmogPython 'dev-env.py' (@('--check') + $targetArguments)
    Invoke-TransmogPython 'check-crypto-graph.py' @()
}
