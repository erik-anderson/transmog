# OS-lease bridge shared with the Python output-retention command.
. (Join-Path $PSScriptRoot 'python-tools.ps1')

function Start-TestOutputRun([string]$Path, [string]$Group) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = Get-TransmogPython
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in @((Join-Path $PSScriptRoot 'test-output-run.py'), '--path', $Path, '--kind', 'desktop', '--group', $Group)) {
        $start.ArgumentList.Add($argument)
    }
    $lease = [Diagnostics.Process]::Start($start)
    try {
        $ready = $lease.StandardOutput.ReadLineAsync()
        if (-not $ready.Wait(10000) -or -not $ready.Result -or -not ($ready.Result | ConvertFrom-Json).ready) {
            throw 'Test-output lease could not start.'
        }
        return $lease
    } catch {
        $lease.StandardInput.Close()
        if (-not $lease.WaitForExit(10000)) { $lease.Kill($true) }
        $lease.Dispose()
        throw
    }
}

function Complete-TestOutputRun($Lease, [bool]$Passed, [bool]$Completed) {
    try {
        if ($Completed) { $Lease.StandardInput.WriteLine((@{passed=$Passed} | ConvertTo-Json -Compress)) }
        $Lease.StandardInput.Close()
        if (-not $Lease.WaitForExit(10000)) { $Lease.Kill($true); throw 'Test-output lease did not finish.' }
        if ($Lease.ExitCode -ne 0) { throw ('Test-output lease failed: ' + $Lease.StandardError.ReadToEnd()) }
    } finally { $Lease.Dispose() }
}
