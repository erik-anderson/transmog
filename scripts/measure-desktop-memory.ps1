param(
    [Parameter(Mandatory)]
    [ValidateRange(1, 2147483647)]
    [int]$ProbeProcessId
)

$ErrorActionPreference = 'Stop'
$measuredProcess = Get-Process -Id $ProbeProcessId -ErrorAction Stop
[ordered]@{
    processId = $measuredProcess.Id
    startedAtUtc = $measuredProcess.StartTime.ToUniversalTime().ToString('o')
    sampledAtUtc = [DateTimeOffset]::UtcNow.ToString('o')
    workingSetBytes = $measuredProcess.WorkingSet64
    privateBytes = $measuredProcess.PrivateMemorySize64
} | ConvertTo-Json -Compress
