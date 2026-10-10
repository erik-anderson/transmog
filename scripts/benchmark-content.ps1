# Convenience entry point; sampling and reports live in Python.
[CmdletBinding()]
param(
    [ValidateRange(1,[long]::MaxValue)][long]$PeakWorkingSetLimitBytes = 512MB,
    [ValidateRange(1,1000)][int]$SampleIntervalMilliseconds = 5,
    [string]$OutputPath = 'artifacts/performance/content-pipeline.json'
)
. (Join-Path $PSScriptRoot 'python-tools.ps1')
Invoke-TransmogPython 'benchmark-content.py' @('--peak-working-set-limit-bytes', $PeakWorkingSetLimitBytes, '--sample-interval-milliseconds', $SampleIntervalMilliseconds, '--output-path', $OutputPath)
