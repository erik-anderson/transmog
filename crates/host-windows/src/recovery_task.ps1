# Invoked with structured input, never interpolated into PowerShell source.
$i = $env:TRANSMOG_INPUT | ConvertFrom-Json
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$source = 'Transmog.ProxyRecovery.v1'
$powerShell = Join-Path ([Environment]::GetFolderPath('Windows')) 'System32\WindowsPowerShell\v1.0\powershell.exe'
function Get-AccountSid([string]$account) {
    if ($account.StartsWith('S-1-')) { return $account }
    return ([Security.Principal.NTAccount]::new($account)).Translate([Security.Principal.SecurityIdentifier]).Value
}
function Get-RecoveryName([string]$journal) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $hash = [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($journal.ToUpperInvariant()))).Replace('-', '')
        return "Transmog Proxy Recovery $sid $hash"
    } finally { $sha.Dispose() }
}
function Get-RecoveryAction([string]$journal, [string]$executable) {
    $payload = [ordered]@{ journal = $journal; executable = $executable } | ConvertTo-Json -Compress
    $encodedPayload = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($payload))
    # Scheduler expands environment variables in both ExecAction.Path and
    # Arguments. Encode the data and launch without a shell so literal %NAME%
    # and $(Arg0) in valid Windows filenames cannot change the action.
    $launcher = @'
$ErrorActionPreference = 'Stop'
$payload = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__PAYLOAD__')) | ConvertFrom-Json
$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = $payload.executable
$start.Arguments = '--recover-proxy "' + $payload.journal + '"'
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
$process = [Diagnostics.Process]::Start($start)
$process.WaitForExit()
exit $process.ExitCode
'@
    $launcher = $launcher.Replace('__PAYLOAD__', $encodedPayload)
    return [pscustomobject]@{
        Path = $powerShell
        Arguments = '-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand ' + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($launcher))
    }
}
function Get-OwnedPayload($candidate) {
    try {
        $definition = $candidate.Definition
        if ($definition.RegistrationInfo.Source -cne $source -or
            (Get-AccountSid $definition.Principal.UserId) -cne $sid -or
            $definition.Actions.Count -ne 1 -or $definition.Actions.Item(1).Type -ne 0) { return $null }
        $action = $definition.Actions.Item(1)
        if ($definition.RegistrationInfo.Documentation) {
            $data = $definition.RegistrationInfo.Documentation | ConvertFrom-Json
            if ($data.schema -ne 1 -or -not [IO.Path]::IsPathRooted([string]$data.journal) -or
                -not [IO.Path]::IsPathRooted([string]$data.executable)) { return $null }
            # Compare with the action saved at registration, not this version's
            # launcher source. Updates and line-ending changes must not strand
            # an otherwise valid task from a previous executable.
            if (-not $data.actionPath -or -not $data.actionArguments -or
                $action.Path -ine $data.actionPath -or $action.Arguments -cne $data.actionArguments) { return $null }
        } else {
            # Recognize earlier direct-executable tasks for safe maintenance.
            $prefix = '--recover-proxy "'
            if (-not $action.Arguments.StartsWith($prefix) -or -not $action.Arguments.EndsWith('"')) { return $null }
            $data = [pscustomobject]@{
                journal = $action.Arguments.Substring($prefix.Length, $action.Arguments.Length - $prefix.Length - 1)
                executable = $action.Path
            }
            if (-not [IO.Path]::IsPathRooted($data.journal) -or -not [IO.Path]::IsPathRooted($data.executable)) { return $null }
        }
        if ($candidate.Name -cne (Get-RecoveryName $data.journal)) { return $null }
        return $data
    } catch { return $null }
}
$name = Get-RecoveryName ([string]$i.journal)
if ($i.operation -eq 'name') { $name; exit 0 }
$service = New-Object -ComObject 'Schedule.Service'
$service.Connect()
$folder = $service.GetFolder('\')
if ($i.operation -eq 'journals') {
    $journals = @(
        foreach ($candidate in $folder.GetTasks(1)) { # Include hidden owned tasks.
            $data = Get-OwnedPayload $candidate
            if ($null -ne $data -and $data.executable -ieq [string]$i.executable) { $data.journal }
        }
    )
    ConvertTo-Json -InputObject $journals -Compress
    exit 0
}
$existing = $null
try {
    $existing = $folder.GetTask($name)
} catch {
    if ($_.Exception.HResult -ne -2147024894) { throw }
}
if ($null -ne $existing) {
    $data = Get-OwnedPayload $existing
    if ($null -eq $data -or $data.journal -ine [string]$i.journal) {
        throw 'The recovery task name belongs to another task.'
    }
}
if ($i.operation -eq 'remove') {
    if ($null -ne $existing) { $folder.DeleteTask($name, 0) }
    exit 0
}
if ($i.operation -ne 'register') { throw 'Unknown recovery task operation.' }
$task = $service.NewTask(0)
$task.RegistrationInfo.Source = $source
$task.RegistrationInfo.Description = 'Restore proxy settings after an interrupted Transmog capture.'
$launch = Get-RecoveryAction ([string]$i.journal) ([string]$i.executable)
$task.RegistrationInfo.Documentation = [ordered]@{
    schema = 1; journal = [string]$i.journal; executable = [string]$i.executable
    actionPath = $launch.Path; actionArguments = $launch.Arguments
} | ConvertTo-Json -Compress
$task.Principal.UserId = $sid
$task.Principal.LogonType = 3 # TASK_LOGON_INTERACTIVE_TOKEN: no password or batch-logon right.
$task.Principal.RunLevel = 0 # TASK_RUNLEVEL_LUA: no elevation.
$trigger = $task.Triggers.Create(9) # TASK_TRIGGER_LOGON, scoped explicitly for standard users.
$trigger.UserId = $sid
$action = $task.Actions.Create(0) # TASK_ACTION_EXEC.
$action.Path = $launch.Path
$action.Arguments = $launch.Arguments
$task.Settings.Enabled = $true
$task.Settings.AllowDemandStart = $true
$task.Settings.DisallowStartIfOnBatteries = $false
$task.Settings.StopIfGoingOnBatteries = $false
$task.Settings.RunOnlyIfIdle = $false
$task.Settings.RunOnlyIfNetworkAvailable = $false
$task.Settings.StartWhenAvailable = $true
$task.Settings.MultipleInstances = 2 # TASK_INSTANCES_IGNORE_NEW.
$task.Settings.ExecutionTimeLimit = 'PT1M'
$task.Settings.RestartCount = 3
$task.Settings.RestartInterval = 'PT1M'
$null = $folder.RegisterTaskDefinition($name, $task, 6, $sid, $null, 3) # CREATE_OR_UPDATE.
