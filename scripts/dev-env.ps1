[CmdletBinding()]
param([switch]$Check)
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$environmentJson = & (Get-TransmogPython) (Join-Path $PSScriptRoot 'dev-env.py') --emit-env
if ($LASTEXITCODE -ne 0) { throw 'Development environment configuration failed.' }
$environment = ($environmentJson -join "`n") | ConvertFrom-Json -AsHashtable
foreach ($entry in $environment.GetEnumerator()) {
    [Environment]::SetEnvironmentVariable($entry.Key, [string]$entry.Value, 'Process')
}
if ($Check) {
    Invoke-TransmogPython 'dev-env.py' @('--check')
    Invoke-TransmogPython 'check-crypto-graph.py' @()
}
