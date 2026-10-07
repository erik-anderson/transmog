param()
. (Join-Path $PSScriptRoot 'windows-release-common.ps1')
$priorNativePreference = $PSNativeCommandUseErrorActionPreference
$priorExitCode = $global:LASTEXITCODE
try {
    $PSNativeCommandUseErrorActionPreference = $false
    $denial = (& (Get-Process -Id $PID).Path -NoProfile -Command 'Write-Output "HTTP 403 Forbidden"; exit 1' 2>&1 | Out-String)
    $denialExit = $LASTEXITCODE
    if ($denialExit -ne 1) { throw 'The native permission-denial fixture did not fail.' }
    Assert-OtherProfileSigningDenied -ExitCode $denialExit -Output $denial
    if ($global:LASTEXITCODE -ne 0) { throw 'The expected denial would fail the GitHub PowerShell wrapper.' }
    foreach ($case in @(
        @{ ExitCode = 0; Output = 'HTTP 403 Forbidden'; Name = 'successful signing' },
        @{ ExitCode = 1; Output = 'network timeout'; Name = 'an unrelated signing failure' }
    )) {
        $rejected = $false
        try { Assert-OtherProfileSigningDenied -ExitCode $case.ExitCode -Output $case.Output } catch { $rejected = $true }
        if (-not $rejected) { throw "The permission test accepted $($case.Name)." }
    }
    Write-Host 'Expected native denial clears the shell exit code; unexpected results remain rejected.'
} finally {
    $PSNativeCommandUseErrorActionPreference = $priorNativePreference
    $global:LASTEXITCODE = $priorExitCode
}
