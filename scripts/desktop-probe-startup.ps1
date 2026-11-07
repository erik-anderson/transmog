function Wait-DesktopProbePage {
    param(
        [Parameter(Mandatory)]$Process,
        [Parameter(Mandatory)][string]$ProfileRoot,
        [int]$Port = 9333,
        [switch]$DiscoverPort,
        [ValidateRange(1, 30)][int]$TimeoutSeconds = 30
    )
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $targets = @()
    $attempts = 0
    $lastError = $null
    $reason = 'deadline'
    do {
        if ($Process.HasExited) { $reason = 'process-exited'; break }
        if ($DiscoverPort) {
            $endpoint = Get-ChildItem -LiteralPath $ProfileRoot -Filter DevToolsActivePort -Recurse -File | Select-Object -First 1
            if ($endpoint) {
                $discovered = 0
                if ([int]::TryParse((Get-Content -LiteralPath $endpoint.FullName -First 1), [ref]$discovered) -and $discovered -gt 0 -and $discovered -le 65535) { $Port = $discovered }
            }
        }
        $attempts++
        try {
            $response = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/json/list" -TimeoutSec 1 -ErrorAction Stop
            $targets = @($response)
            $lastError = $null
        } catch {
            $targets = @()
            $lastError = $_.Exception.Message
        }
        # DevTools can publish a worker or an initial untitled page before the
        # application document is ready. Match the same page the smoke uses.
        $pages = @($targets | Where-Object { $_.type -ceq 'page' -and $_.title -ceq 'Transmog' -and $_.webSocketDebuggerUrl })
        if ($pages.Count -gt 1) { $reason = 'ambiguous-pages'; break }
        if ($pages.Count -eq 1) {
            return [pscustomobject]@{ Ready = $true; Port = $Port; Reason = 'ready'; Attempts = $attempts; Targets = $targets; LastError = $lastError }
        }
        if ([DateTime]::UtcNow -ge $deadline) { break }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    return [pscustomobject]@{ Ready = $false; Port = $Port; Reason = $reason; Attempts = $attempts; Targets = $targets; LastError = $lastError }
}
