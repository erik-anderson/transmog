[CmdletBinding()]
param(
    [ValidateRange(1, [long]::MaxValue)]
    [long]$PeakWorkingSetLimitBytes = 512MB,

    [ValidateRange(1, 1000)]
    [int]$SampleIntervalMilliseconds = 5,

    [string]$OutputPath = 'artifacts/performance/content-pipeline.json'
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
. "$PSScriptRoot/dev-env.ps1"

function Assert-LastCommand([string]$Description) {
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE."
    }
}

function Measure-ChildProcess {
    param(
        [Parameter(Mandatory)]
        [string]$FilePath,

        [Parameter(Mandatory)]
        [string]$Argument
    )

    $standardOutput = [System.IO.Path]::GetTempFileName()
    $standardError = [System.IO.Path]::GetTempFileName()
    try {
        $process = Start-Process `
            -FilePath $FilePath `
            -ArgumentList $Argument `
            -RedirectStandardOutput $standardOutput `
            -RedirectStandardError $standardError `
            -NoNewWindow `
            -PassThru
        $sampledPeak = 0L
        while (-not $process.HasExited) {
            try {
                $process.Refresh()
                $sampledPeak = [Math]::Max($sampledPeak, [long]$process.WorkingSet64)
            } catch [System.InvalidOperationException] {
                # The process can exit between HasExited and Refresh.
            }
            Start-Sleep -Milliseconds $SampleIntervalMilliseconds
        }
        $process.WaitForExit()
        if ($process.ExitCode -ne 0) {
            $errorText = Get-Content -LiteralPath $standardError -Raw
            throw "Benchmark child $Argument failed with exit code $($process.ExitCode): $errorText"
        }
        $json = Get-Content -LiteralPath $standardOutput -Raw | ConvertFrom-Json
        return [pscustomobject]@{
            sampledPeakWorkingSetBytes = $sampledPeak
            output = $json
        }
    } finally {
        Remove-Item -LiteralPath $standardOutput, $standardError -Force -ErrorAction SilentlyContinue
    }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $repoRoot
try {
    cargo build --locked --release -p transmog-content --example content_pipeline_benchmark
    Assert-LastCommand 'Building the content-pipeline benchmark'

    $extension = if ($IsWindows) { '.exe' } else { '' }
    $benchmark = Join-Path $repoRoot "target/release/examples/content_pipeline_benchmark$extension"
    if (-not (Test-Path -LiteralPath $benchmark -PathType Leaf)) {
        throw "Benchmark executable was not found at $benchmark."
    }

    $benchmarkReport = (& $benchmark --json | Out-String) | ConvertFrom-Json
    Assert-LastCommand 'Running content-pipeline throughput, first-byte, and cancellation benchmarks'
    if ($benchmarkReport.throughput.Count -ne 8 -or $benchmarkReport.firstByte.Count -ne 8) {
        throw 'Benchmark did not report all coding/output combinations.'
    }
    foreach ($metric in $benchmarkReport.throughput) {
        if ($metric.bytesPerSecond -le 0 -or $metric.nanosPerOperation -le 0) {
            throw "Invalid throughput accounting for $($metric.coding)/$($metric.output)."
        }
    }
    if ($benchmarkReport.cancellation.maxNanos -gt $benchmarkReport.cancellation.deadlineNanos) {
        throw 'Cancellation responsiveness exceeded its explicit deadline.'
    }
    foreach ($metric in $benchmarkReport.firstByte) {
        if ($metric.encodedBytesBeforeOutput -gt $metric.encodedBodyBytes) {
            throw "Invalid first-byte accounting for $($metric.coding)/$($metric.output)."
        }
    }

    $baseline = Measure-ChildProcess -FilePath $benchmark -Argument '--memory-baseline'
    $workload = Measure-ChildProcess -FilePath $benchmark -Argument '--memory-probe'
    if ($baseline.sampledPeakWorkingSetBytes -le 0 -or $workload.sampledPeakWorkingSetBytes -le 0) {
        throw 'The process sampler did not capture a positive working-set measurement.'
    }
    if ($workload.sampledPeakWorkingSetBytes -gt $PeakWorkingSetLimitBytes) {
        throw "Sampled peak working set $($workload.sampledPeakWorkingSetBytes) exceeded $PeakWorkingSetLimitBytes bytes."
    }
    $incrementalPeak = [Math]::Max(
        0L,
        $workload.sampledPeakWorkingSetBytes - $baseline.sampledPeakWorkingSetBytes
    )

    $commit = (& git rev-parse HEAD | Out-String).Trim()
    Assert-LastCommand 'Reading the Git revision'
    $workingTreeDirty = [bool](& git status --porcelain)
    Assert-LastCommand 'Reading the Git working-tree state'
    $report = [ordered]@{
        schemaVersion = 1
        recordedAtUtc = [DateTimeOffset]::UtcNow.ToString('O')
        commit = $commit
        workingTreeDirty = $workingTreeDirty
        operatingSystem = [System.Runtime.InteropServices.RuntimeInformation]::OSDescription
        architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
        benchmark = $benchmarkReport
        memory = [ordered]@{
            sampleIntervalMilliseconds = $SampleIntervalMilliseconds
            peakWorkingSetLimitBytes = $PeakWorkingSetLimitBytes
            baselineSampledPeakWorkingSetBytes = $baseline.sampledPeakWorkingSetBytes
            workloadSampledPeakWorkingSetBytes = $workload.sampledPeakWorkingSetBytes
            incrementalSampledPeakWorkingSetBytes = $incrementalPeak
            baseline = $baseline.output
            workload = $workload.output
        }
    }

    $resolvedOutput = Join-Path $repoRoot $OutputPath
    $outputDirectory = Split-Path -Parent $resolvedOutput
    New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
    $report | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $resolvedOutput -Encoding utf8
    Write-Output ($report | ConvertTo-Json -Depth 10)
    Write-Output "Saved benchmark report to $resolvedOutput"
} finally {
    Pop-Location
}
