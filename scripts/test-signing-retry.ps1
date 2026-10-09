param()
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'windows-signing-retry.ps1')
$priorExitCode = $global:LASTEXITCODE
$priorNativePreference = $PSNativeCommandUseErrorActionPreference
try {
    foreach ($case in @(
        @{ Name='immediate success'; Codes=@(0); Message='Signed'; Attempts=1; Delays=@(); Reject=$false },
        @{ Name='temporary timestamp outage'; Codes=@(1,1,0); Message='The specified timestamp server either could not be reached or returned an invalid response.'; Attempts=3; Delays=@(10,20); Reject=$false },
        @{ Name='persistent timestamp outage'; Codes=@(1,1,1); Message='The specified timestamp server either could not be reached or returned an invalid response.'; Attempts=3; Delays=@(10,20); Reject=$true },
        @{ Name='permission denial'; Codes=@(1); Message='HTTP 403 Forbidden'; Attempts=1; Delays=@(); Reject=$true },
        @{ Name='another signing failure'; Codes=@(1); Message='Invalid signing input'; Attempts=1; Delays=@(); Reject=$true }
    )) {
        $fixture = [pscustomobject]@{ Calls=0; Delays=[Collections.Generic.List[int]]::new() }
        $failure = $null
        try {
            Invoke-SigningWithTimestampRetry -Operation {
                $code = $case.Codes[$fixture.Calls++]
                [pscustomobject]@{ ExitCode=$code; Output=$case.Message }
            } -Delay { param($Seconds) $fixture.Delays.Add($Seconds) }
        } catch { $failure = $_ }
        if (($null -ne $failure) -ne $case.Reject -or $fixture.Calls -ne $case.Attempts -or
            ($fixture.Delays -join ',') -cne ($case.Delays -join ',')) { throw "Wrong retry behavior: $($case.Name)" }
        if (-not $case.Reject -and $global:LASTEXITCODE -ne 0) { throw 'A successful retry left the native shell failure code set.' }
        if ($PSNativeCommandUseErrorActionPreference -ne $priorNativePreference) { throw 'Retry changed the caller native-error preference.' }
        Write-Host "Passed $($case.Name)."
    }
} finally {
    $global:LASTEXITCODE = $priorExitCode
    $PSNativeCommandUseErrorActionPreference = $priorNativePreference
}
