# A controlled task within the production one-minute budget. No application
# execution or proxy changes; this qualifies the observer, not the application.
$ErrorActionPreference = 'Stop'
$out = Join-Path $PWD 'artifacts/proxy-recovery-budget'
$null = [IO.Directory]::CreateDirectory($out)
function Save-Json($name, $value) { [IO.File]::WriteAllText((Join-Path $out $name), ($value | ConvertTo-Json -Depth 12), [Text.UTF8Encoding]::new($false)) }
trap { Save-Json 'error.json' @{ message = $_.ToString(); position = $_.InvocationInfo.PositionMessage }; break }
$originalSource = git show '37e8290627dd38faf7279b3d592cc0bd368d1813:apps/desktop/tests/proxy_recovery_task.rs'
if ($LASTEXITCODE -ne 0) { throw 'Original test source unavailable.' }
$match = [regex]::Match(($originalSource -join "`n"), '(?s)const RUN_TASK: &str = r"(.*?)";')
if (-not $match.Success -or -not $match.Groups[1].Value.Contains('AddSeconds(30)')) { throw 'Original observer not found.' }
$original = $match.Groups[1].Value
$proposed = [IO.File]::ReadAllText((Join-Path $PWD 'scripts/probe-proposed-recovery-task-wait.ps1'))
$helper = (Resolve-Path 'crates/host-windows/src/recovery_task.ps1').Path
$temporaryRoot = [IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')
$fixture = Join-Path $temporaryRoot ('transmog-task-budget-' + [Guid]::NewGuid().ToString('N'))
$null = [IO.Directory]::CreateDirectory($fixture)
$journal = Join-Path $fixture 'proxy-recovery.json'
$env:TRANSMOG_INPUT = @{ operation = 'register'; journal = $journal; executable = "$env:WINDIR/System32/WindowsPowerShell/v1.0/powershell.exe" } | ConvertTo-Json -Compress
$powerShell = "$env:WINDIR/System32/WindowsPowerShell/v1.0/powershell.exe"
& $powerShell -NoLogo -NoProfile -NonInteractive -File $helper
if ($LASTEXITCODE -ne 0) { throw 'Owned task registration failed.' }
$nameHash = [Security.Cryptography.SHA256]::Create()
try { $name = 'Transmog Proxy Recovery ' + [Security.Principal.WindowsIdentity]::GetCurrent().User.Value + ' ' + [BitConverter]::ToString($nameHash.ComputeHash([Text.Encoding]::UTF8.GetBytes($journal.ToUpperInvariant()))).Replace('-', '') }
finally { $nameHash.Dispose() }
$env:TRANSMOG_TEST_TASK = $name
$service = New-Object -ComObject Schedule.Service; $service.Connect(); $folder = $service.GetFolder('\')
function Observe($body) {
    $start = [Diagnostics.ProcessStartInfo]::new(); $start.FileName = $powerShell
    $start.Arguments = '-NoLogo -NoProfile -NonInteractive -EncodedCommand ' + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($body))
    $start.UseShellExecute = $false; $start.CreateNoWindow = $true; $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.RedirectStandardOutput = $true; $start.RedirectStandardError = $true
    $watch = [Diagnostics.Stopwatch]::StartNew(); $process = [Diagnostics.Process]::Start($start)
    $stdout = $process.StandardOutput.ReadToEndAsync(); $stderr = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(90000)) { $process.Kill(); $process.WaitForExit(); throw 'Observer exceeded its independent diagnostic bound.' }
    return @{ exitCode = $process.ExitCode; seconds = $watch.Elapsed.TotalSeconds; stdout = $stdout.Result; stderr = $stderr.Result }
}
try {
    $definition = $folder.GetTask($name).Definition
    if ($definition.Settings.ExecutionTimeLimit -cne 'PT1M') { throw 'Production task limit changed.' }
    $action = $definition.Actions.Item(1)
    $action.Arguments = '-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand ' + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes('Start-Sleep -Seconds 35; exit 0'))
    $data = $definition.RegistrationInfo.Documentation | ConvertFrom-Json
    $data.actionArguments = $action.Arguments
    $definition.RegistrationInfo.Documentation = $data | ConvertTo-Json -Compress
    $null = $folder.RegisterTaskDefinition($name, $definition, 6, $definition.Principal.UserId, $null, 3)
    $task = $folder.GetTask($name)
    $old = Observe $original
    $old.taskStateAtRejection = $task.State; $old.taskResultAtRejection = $task.LastTaskResult
    Save-Json 'original-observer.json' $old
    if ($old.exitCode -eq 0 -or -not $old.stderr.Contains('The recovery task did not finish.') -or $old.taskStateAtRejection -ne 4) { throw 'Original observer did not reproduce premature rejection.' }
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ($task.GetInstances(0).Count -gt 0 -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
    if ($task.GetInstances(0).Count -gt 0 -or $task.LastTaskResult -ne 0) { throw 'Controlled task did not complete successfully within its original one-minute limit.' }
    $env:TRANSMOG_TEST_TASK_DIAGNOSTICS = Join-Path $out 'proposed-observations'
    $new = Observe $proposed
    Save-Json 'proposed-observer.json' $new
    if ($new.exitCode -ne 0 -or $new.seconds -lt 35 -or $task.LastTaskResult -ne 0) { throw 'Proposed observer did not require successful delayed completion.' }
    Save-Json 'result.json' @{ source = $env:GITHUB_SHA; run = $env:GITHUB_RUN_ID; originalSource = '37e8290627dd38faf7279b3d592cc0bd368d1813'; controlledActionSeconds = 35; productionExecutionLimit = $task.Definition.Settings.ExecutionTimeLimit; originalRejectedBeforeSuccessfulCompletion = $true; proposedRequiredSuccessfulCompletion = $true; noApplicationExecution = $true; noProxyWrites = $true }
} finally {
    try { $folder.GetTask($name).Stop(0) } catch { }
    $env:TRANSMOG_INPUT = @{ operation = 'remove'; journal = $journal; executable = $powerShell } | ConvertTo-Json -Compress
    & $powerShell -NoLogo -NoProfile -NonInteractive -File $helper
    if ($LASTEXITCODE -ne 0) { throw 'Owned task removal failed.' }
    $resolved = [IO.Path]::GetFullPath($fixture)
    if (-not $resolved.StartsWith($temporaryRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe fixture cleanup path.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
