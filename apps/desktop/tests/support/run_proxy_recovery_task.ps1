$ErrorActionPreference = 'Stop'
$service = New-Object -ComObject 'Schedule.Service'
$service.Connect()
$task = $service.GetFolder('\').GetTask($env:TRANSMOG_TEST_TASK)
$definition = $task.Definition
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
function Get-AccountSid([string]$account) {
    if ($account.StartsWith('S-1-')) { return $account }
    return ([Security.Principal.NTAccount]::new($account)).Translate([Security.Principal.SecurityIdentifier]).Value
}
if ((Get-AccountSid $definition.Principal.UserId) -cne $sid -or $definition.Principal.LogonType -ne 3 -or $definition.Principal.RunLevel -ne 0) { throw 'Recovery must run as the current user without elevation or a password.' }
if ($definition.Triggers.Count -ne 1 -or $definition.Triggers.Item(1).Type -ne 9 -or (Get-AccountSid $definition.Triggers.Item(1).UserId) -cne $sid) { throw 'Expected a logon trigger scoped to this user.' }
if ($definition.Settings.DisallowStartIfOnBatteries -or $definition.Settings.StopIfGoingOnBatteries -or $definition.Settings.RunOnlyIfNetworkAvailable -or $definition.Settings.RunOnlyIfIdle) { throw 'Recovery must run on battery without networking or idle requirements.' }
if ($definition.Actions.Item(1).Arguments -notlike '*-WindowStyle Hidden -EncodedCommand *') { throw 'Expected the encoded, hidden headless recovery action.' }
if ($definition.Settings.ExecutionTimeLimit -cne 'PT1M') { throw 'Expected the one-minute recovery task execution limit.' }
# The test must allow the task's full existing execution budget, plus a bounded
# margin for dispatch and completion reporting. This does not extend the task.
$timeoutSeconds = 75
$watch = [Diagnostics.Stopwatch]::StartNew()
$samples = @()
$lastState = $null
$nextSample = 0
$taskXml = $task.Xml
function Test-TaskError($record, [int]$code) {
    for ($exception = $record.Exception; $null -ne $exception; $exception = $exception.InnerException) {
        if ($exception.HResult -eq $code) { return $true }
    }
    return $false
}
function Get-TaskProcesses {
    if (-not $definition.RegistrationInfo.Documentation) { return @() }
    $executable = ($definition.RegistrationInfo.Documentation | ConvertFrom-Json).executable
    @([Diagnostics.Process]::GetProcessesByName([IO.Path]::GetFileNameWithoutExtension($executable)) | ForEach-Object {
        try {
            if ($_.MainModule.FileName -ieq $executable) {
                @{ pid = $_.Id; session = $_.SessionId; cpuSeconds = $_.TotalProcessorTime.TotalSeconds; workingSet = $_.WorkingSet64 }
            }
        } catch { } # A process may exit while the diagnostic snapshot is read.
    })
}
function Save-Observation([string]$result, $lastResult) {
    $record = [ordered]@{
        schema = 1; result = $result; source = $env:GITHUB_SHA; run = $env:GITHUB_RUN_ID
        attempt = $env:GITHUB_RUN_ATTEMPT; target = $env:CARGO_BUILD_TARGET
        task = $env:TRANSMOG_TEST_TASK; executionLimit = $definition.Settings.ExecutionTimeLimit
        observerLimitSeconds = $timeoutSeconds; elapsedSeconds = $watch.Elapsed.TotalSeconds
        lastTaskResult = $lastResult; samples = $samples; definition = $taskXml
    }
    $json = $record | ConvertTo-Json -Depth 8
    if ($env:TRANSMOG_TEST_TASK_DIAGNOSTICS) {
        $null = [IO.Directory]::CreateDirectory($env:TRANSMOG_TEST_TASK_DIAGNOSTICS)
        $name = if ($result -eq 'completed') { [Guid]::NewGuid().ToString('N') + '.json' } else { 'failure.json' }
        [IO.File]::WriteAllText((Join-Path $env:TRANSMOG_TEST_TASK_DIAGNOSTICS $name), $json, [Text.UTF8Encoding]::new($false))
    }
    if ($result -ne 'completed') { [Console]::Error.WriteLine($json) }
}
# Invoke IRegisteredTask.Run, rather than starting the action's executable ourselves.
$running = $task.Run($null)
do {
    Start-Sleep -Milliseconds 100
    try {
        $running.Refresh()
        $active = $running.State -in @(2, 4) # Queued or running.
        if ($running.State -ne $lastState -or $watch.Elapsed.TotalSeconds -ge $nextSample) {
            $samples += @{ seconds = $watch.Elapsed.TotalSeconds; state = $running.State; enginePid = $running.EnginePID; processes = @(Get-TaskProcesses) }
            $lastState = $running.State
            $nextSample = $watch.Elapsed.TotalSeconds + 5
        }
    } catch {
        if (-not (Test-TaskError $_ -2147216629)) { throw } # SCHED_E_TASK_NOT_RUNNING.
        $active = $false
    }
} while ($active -and $watch.Elapsed.TotalSeconds -lt $timeoutSeconds)
if ($active) {
    Save-Observation 'deadline' $task.LastTaskResult
    throw 'The recovery task did not finish within its execution and reporting budget.'
}
# A successful recovery may remove its own registration. Only the documented
# missing-task error is acceptable; any other Scheduler error still fails.
$lastResult = $null
do {
    try { $lastResult = $service.GetFolder('\').GetTask($env:TRANSMOG_TEST_TASK).LastTaskResult }
    catch { if (-not (Test-TaskError $_ -2147024894)) { throw }; $lastResult = $null }
    $reportPending = $null -ne $lastResult -and $lastResult -in @(267009, 267011) # Running or not-yet-run receipt.
    if ($reportPending -and $watch.Elapsed.TotalSeconds -lt $timeoutSeconds) { Start-Sleep -Milliseconds 100 }
} while ($reportPending -and $watch.Elapsed.TotalSeconds -lt $timeoutSeconds)
if ($reportPending) {
    Save-Observation 'deadline' $lastResult
    throw 'The recovery task completion report did not settle within the reporting budget.'
}
if ($null -ne $lastResult -and $lastResult -ne 0) {
    Save-Observation 'failed' $lastResult
    throw "The recovery task exited unsuccessfully: $lastResult"
}
Save-Observation 'completed' $lastResult
