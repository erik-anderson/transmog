function Invoke-SigningWithTimestampRetry {
    param([Parameter(Mandatory)][scriptblock]$Operation,
        [scriptblock]$Delay = { param($Seconds) Start-Sleep -Seconds $Seconds })
    $priorNativePreference = $PSNativeCommandUseErrorActionPreference
    try {
        $PSNativeCommandUseErrorActionPreference = $false
        for ($attempt = 1; $attempt -le 3; $attempt++) {
            $result = & $Operation
            $global:LASTEXITCODE = [int]$result.ExitCode
            if ($result.Output) { Write-Host ([string]$result.Output).TrimEnd() }
            if ($result.ExitCode -eq 0) { return }
            $timestampUnavailable = [string]$result.Output -match '(?is)timestamp server.*(?:could not be reached|invalid response)'
            if (-not $timestampUnavailable -or $attempt -eq 3) {
                throw "Artifact Signing failed after $attempt attempt(s): $($result.ExitCode)"
            }
            $seconds = 10 * $attempt
            Write-Host "Timestamp service failed; retrying the same signing input in $seconds seconds (attempt $($attempt + 1) of 3)."
            & $Delay $seconds
        }
    } finally { $PSNativeCommandUseErrorActionPreference = $priorNativePreference }
}
