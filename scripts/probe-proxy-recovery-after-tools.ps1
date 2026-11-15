param([ValidateSet('before-tools', 'after-tools')][string]$Phase)
# Diagnostic observation only: preserve the production task's one-minute limit.
$ErrorActionPreference = 'Stop'
$out = Join-Path $PWD ('artifacts/proxy-recovery-after-tools/' + $Phase)
$null = New-Item -ItemType Directory -Force -Path $out
function Save-Json($name, $value) { $value | ConvertTo-Json -Depth 25 | Set-Content -LiteralPath (Join-Path $out $name) -Encoding UTF8 }
trap { Save-Json 'error.json' @{ message = $_.ToString(); position = $_.InvocationInfo.PositionMessage }; break }
function Hash-File($path) {
    $stream = [IO.File]::OpenRead($path); $sha = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '').ToLowerInvariant() }
    finally { $stream.Dispose(); $sha.Dispose() }
}
function Proxy-Snapshot {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Internet Settings')
    try { return (@{ enabled = $key.GetValue('ProxyEnable', $null); server = $key.GetValue('ProxyServer', $null); bypass = $key.GetValue('ProxyOverride', $null) } | ConvertTo-Json -Compress) }
    finally { $key.Dispose() }
}
$debug = (Resolve-Path 'artifacts/native-debug/proxy-recovery-task-native-probe/debug-transmog.exe').Path
$expected = '20f90f698dde593cdd888002996918ac5023c4b8575f69efc183eb60a1c3154e'
if ((Hash-File $debug) -cne $expected) { throw 'Retained debug executable hash mismatch.' }
$header = [IO.File]::ReadAllBytes($debug); $pe = [BitConverter]::ToInt32($header, 0x3c)
if ([BitConverter]::ToUInt16($header, $pe + 4) -ne 0xaa64 -or [BitConverter]::ToUInt16($header, $pe + 24 + 68) -ne 3) { throw 'Expected the original native ARM64 console debug executable.' }
$script = (Resolve-Path 'crates/host-windows/src/recovery_task.ps1').Path
$original = git show '37e8290627dd38faf7279b3d592cc0bd368d1813:crates/host-windows/src/recovery_task.ps1'
if ($LASTEXITCODE -ne 0 -or ($original -join "`n").TrimEnd() -cne (Get-Content -Raw -LiteralPath $script).Replace("`r`n", "`n").TrimEnd()) { throw 'Production launcher differs from the failed release source.' }
Save-Json 'identity.json' @{ diagnosticSource = $env:GITHUB_SHA; diagnosticRun = $env:GITHUB_RUN_ID; debugProducerSource = '673364e3fe626d52a4ed16e585e031e9da7130c3'; debugProducerRun = 43657175707; debugProducerArtifact = 14139171363; debugHash = $expected; debugLength = $header.Length; productionCodeSource = '37e8290627dd38faf7279b3d592cc0bd368d1813'; taskLimitUnchanged = 'PT1M'; observerSeconds = 70; imageOS = $env:ImageOS; imageVersion = $env:ImageVersion; session = [Diagnostics.Process]::GetCurrentProcess().SessionId; powershell = $PSVersionTable.PSVersion.ToString() }
$before = Proxy-Snapshot
$service = New-Object -ComObject Schedule.Service; $service.Connect(); $folder = $service.GetFolder('\')
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
$records = @()
function Operation($operation) {
    $env:TRANSMOG_INPUT = @{ operation = $operation; journal = $journal; executable = $exe } | ConvertTo-Json -Compress
    & "$env:WINDIR/System32/WindowsPowerShell/v1.0/powershell.exe" -NoLogo -NoProfile -NonInteractive -File $script
    if ($LASTEXITCODE -ne 0) { throw "Production operation failed: $operation" }
}
function Owned-Processes {
    @([Diagnostics.Process]::GetProcessesByName('Transmog recovery test') | ForEach-Object {
        try {
            if ($_.MainModule.FileName -ieq $exe) { @{ pid = $_.Id; session = $_.SessionId; cpuSeconds = $_.TotalProcessorTime.TotalSeconds; workingSet = $_.WorkingSet64; threads = $_.Threads.Count; title = $_.MainWindowTitle } }
        } catch { }
    })
}
function Capture-TaskStreams {
    $definition = $folder.GetTask($name).Definition
    $action = $definition.Actions.Item(1)
    $prefix = '-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand '
    if (-not $action.Arguments.StartsWith($prefix)) { throw 'Unexpected original launcher.' }
    $launcher = [Text.Encoding]::Unicode.GetString([Convert]::FromBase64String($action.Arguments.Substring($prefix.Length)))
    $start = '$process = [Diagnostics.Process]::Start($start)'
    $wait = '$process.WaitForExit()'
    if (-not $launcher.Contains($start) -or -not $launcher.Contains($wait)) { throw 'Original process launch/wait not found.' }
    $captureStart = @'
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$meta = @{ version = $PSVersionTable.PSVersion.ToString(); psHome = $PSHOME; process64Bit = [Environment]::Is64BitProcess; architecture = $env:PROCESSOR_ARCHITECTURE; resolvedPowerShell = (Get-Command powershell.exe -CommandType Application).Source; session = [Diagnostics.Process]::GetCurrentProcess().SessionId }
[IO.File]::WriteAllText([IO.Path]::ChangeExtension($payload.journal, 'launcher.json'), ($meta | ConvertTo-Json -Compress))
$process = [Diagnostics.Process]::Start($start)
$stdout = $process.StandardOutput.ReadToEndAsync()
$stderr = $process.StandardError.ReadToEndAsync()
'@
    $captureWait = @'
$process.WaitForExit()
[IO.File]::WriteAllText([IO.Path]::ChangeExtension($payload.journal, 'stdout.txt'), $stdout.Result)
[IO.File]::WriteAllText([IO.Path]::ChangeExtension($payload.journal, 'stderr.txt'), $stderr.Result)
'@
    $launcher = $launcher.Replace($start, $captureStart).Replace($wait, $captureWait)
    $action.Arguments = $prefix + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($launcher))
    $data = $definition.RegistrationInfo.Documentation | ConvertFrom-Json
    $data.actionArguments = $action.Arguments
    $definition.RegistrationInfo.Documentation = $data | ConvertTo-Json -Compress
    $null = $folder.RegisterTaskDefinition($name, $definition, 6, $definition.Principal.UserId, $null, 3)
}
$bad = $false
for ($fixtureNumber = 1; $fixtureNumber -le 2; $fixtureNumber++) {
    $fixture = Join-Path $temporaryRoot ("Transmog's recovery " + [char]0x3a9 + ' %USERNAME% $(Arg0) ' + [Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Path $fixture
    $exe = Join-Path $fixture 'Transmog recovery test.exe'; $journal = Join-Path $fixture 'proxy-recovery.json'
    $nameHash = [Security.Cryptography.SHA256]::Create()
    try { $name = 'Transmog Proxy Recovery ' + $sid + ' ' + [BitConverter]::ToString($nameHash.ComputeHash([Text.Encoding]::UTF8.GetBytes($journal.ToUpperInvariant()))).Replace('-', '') }
    finally { $nameHash.Dispose() }
    $lease = [IO.File]::Open([IO.Path]::ChangeExtension($journal, 'lock'), [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite, [IO.FileShare]::ReadWrite)
    $lease.Lock(0, 1)
    [IO.File]::WriteAllText($journal, (@{ schema = 1; prior = ($before | ConvertFrom-Json) } | ConvertTo-Json -Depth 5))
    try {
        foreach ($kind in @('cold', 'warm')) {
            Operation 'register'
            Capture-TaskStreams
            $task = $folder.GetTask($name)
            if ($task.Definition.Settings.ExecutionTimeLimit -cne 'PT1M') { throw 'Production task limit changed.' }
            $task.Xml | Set-Content -LiteralPath (Join-Path $out "task-$fixtureNumber-$kind.xml") -Encoding UTF8
            # Register first, then copy immediately before Run. Do not hash or
            # read the fresh destination until the first invocation has ended.
            if ($kind -eq 'cold') { [IO.File]::Copy($debug, $exe, $false) }
            $watch = [Diagnostics.Stopwatch]::StartNew(); $running = $task.Run($null)
            $samples = @(); $lastState = $null; $nextSample = 0
            do {
                Start-Sleep -Milliseconds 100
                try { $running.Refresh(); $state = [int]$running.State; $active = $state -in @(2, 4); $engine = $running.EnginePID }
                catch { if ($_.Exception.HResult -ne -2147216629) { throw }; $state = 'not-running'; $active = $false; $engine = $null }
                if ($state -ne $lastState -or $watch.Elapsed.TotalSeconds -ge $nextSample) {
                    $samples += @{ seconds = $watch.Elapsed.TotalSeconds; state = $state; enginePid = $engine; processes = @(Owned-Processes) }
                    $lastState = $state; $nextSample = $watch.Elapsed.TotalSeconds + 5
                }
            } while ($active -and $watch.Elapsed.TotalSeconds -lt 70)
            $seconds = $watch.Elapsed.TotalSeconds
            $result = $task.LastTaskResult
            $journalRetained = [IO.File]::Exists($journal)
            $record = @{ fixture = $fixtureNumber; kind = $kind; seconds = $seconds; overOriginalThirtySeconds = $seconds -gt 30; activeAtObserverDeadline = $active; lastResult = $result; samples = $samples; liveJournalRetained = $journalRetained; proxyUnchanged = (Proxy-Snapshot) -ceq $before }
            $records += $record; Save-Json 'scheduled.json' $records
            foreach ($suffix in @('stdout.txt', 'stderr.txt', 'launcher.json')) {
                $trace = [IO.Path]::ChangeExtension($journal, $suffix)
                if ([IO.File]::Exists($trace)) { [IO.File]::Copy($trace, (Join-Path $out ('fixture-' + $fixtureNumber + '-' + $kind + '.' + $suffix)), $true) }
            }
            if ($active) { $task.Stop(0) }
            foreach ($owned in @(Owned-Processes)) { Stop-Process -Id $owned.pid -Force }
            if ((Hash-File $exe) -cne $expected) { throw 'Fixture executable changed.' }
            if ($active -or $result -ne 0 -or -not $journalRetained -or -not $record.proxyUnchanged) { $bad = $true }
            Operation 'remove'
        }
    } finally {
        try { $folder.GetTask($name).Stop(0) } catch { }
        foreach ($owned in @(Owned-Processes)) { Stop-Process -Id $owned.pid -Force }
        try { Operation 'remove' } finally { $lease.Dispose() }
        $resolved = [IO.Path]::GetFullPath($fixture)
        if (-not $resolved.StartsWith($temporaryRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe fixture cleanup path.' }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
$unchanged = (Proxy-Snapshot) -ceq $before
Save-Json 'cleanup.json' @{ proxyUnchanged = $unchanged; fixturesRemoved = $true; tasksRemoved = $true }
if ($bad -or -not $unchanged) { throw 'One or more timed debug invocations failed; inspect retained evidence.' }
