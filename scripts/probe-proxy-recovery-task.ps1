# A one-off hosted diagnosis of the exact 0.1.26 task launcher. No builds, UI,
# proxy writes, signing, or reusable native-engine qualification are performed.
$ErrorActionPreference = 'Stop'
$source = '37e8290627dd38faf7279b3d592cc0bd368d1813'
$out = Join-Path $PWD 'artifacts/proxy-recovery-task-probe'
$null = New-Item -ItemType Directory -Force -Path $out
function Save-Json($name, $value) {
    $value | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath (Join-Path $out $name) -Encoding UTF8
}
function Get-ProxySnapshot {
    $key = Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'
    $values = [ordered]@{}
    foreach ($name in @('ProxyEnable', 'ProxyServer', 'ProxyOverride', 'AutoConfigURL')) {
        $values[$name] = $key.$name
    }
    return ($values | ConvertTo-Json -Compress)
}
$payload = (Resolve-Path artifacts/probe-payload).Path
$manifestPath = Join-Path $payload 'manifest.json'
if ((Get-FileHash -LiteralPath $manifestPath).Hash -ine $env:EXPECTED_MANIFEST) { throw 'Original manifest hash mismatch.' }
$manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
if ($manifest.Commit -cne $source -or $manifest.RunId -ne '43648981179' -or $manifest.RunAttempt -ne '1' -or $manifest.Target -cne $env:EXPECTED_TARGET -or $manifest.Gate.Result -cne 'passed') { throw 'Original compiled payload provenance mismatch.' }
foreach ($entry in $manifest.Files.PSObject.Properties) {
    $path = [IO.Path]::GetFullPath((Join-Path $payload $entry.Name))
    if (-not $path.StartsWith($payload + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or (Get-FileHash -LiteralPath $path).Hash -ine $entry.Value) { throw "Compiled file mismatch: $($entry.Name)" }
}
$scriptPath = (Resolve-Path crates/host-windows/src/recovery_task.ps1).Path
$originalScript = git show "${source}:crates/host-windows/src/recovery_task.ps1"
if ($LASTEXITCODE -ne 0 -or (($originalScript -join "`n").TrimEnd() -cne (Get-Content -Raw -LiteralPath $scriptPath).Replace("`r`n", "`n").TrimEnd())) { throw 'Production launcher differs from the failed source.' }
Save-Json 'identity.json' ([ordered]@{
    payloadCommit = $source; payloadRun = 43648981179; payloadManifestHash = $env:EXPECTED_MANIFEST
    diagnosticCommit = $env:GITHUB_SHA; diagnosticRun = $env:GITHUB_RUN_ID
    target = $env:EXPECTED_TARGET; imageOS = $env:ImageOS; imageVersion = $env:ImageVersion
    os = [Environment]::OSVersion.VersionString; architecture = $env:PROCESSOR_ARCHITECTURE
    currentSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    currentSession = [Diagnostics.Process]::GetCurrentProcess().SessionId
    launcherHash = (Get-FileHash -LiteralPath $scriptPath).Hash
    payloadKind = 'optimized compiled artifact; original failure used the debug test executable'
})
& "$env:WINDIR/System32/qwinsta.exe" 2>&1 | Out-String | Set-Content -LiteralPath (Join-Path $out 'sessions.txt')
$before = Get-ProxySnapshot
$tempRoot = [IO.Path]::GetFullPath($env:RUNNER_TEMP)
$fixture = Join-Path $tempRoot ("Transmog's recovery " + [char]0x3a9 + ' %USERNAME% $(Arg0) ' + [Guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $fixture
$exe = Join-Path $fixture 'Transmog recovery test.exe'
Copy-Item -LiteralPath (Join-Path $payload 'binaries/transmog.exe') -Destination $exe
$journal = Join-Path $fixture 'proxy-recovery.json'
# The held overlapping byte-range lock causes recovery to return ProxyInUse
# before reading a journal or writing proxy settings. No journal is necessary.
$lease = [IO.File]::Open([IO.Path]::ChangeExtension($journal, 'lock'), [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite, [IO.FileShare]::ReadWrite)
$lease.Lock(0, 1)
$sha = [Security.Cryptography.SHA256]::Create()
try { $hash = [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($journal.ToUpperInvariant()))).Replace('-', '') } finally { $sha.Dispose() }
$name = 'Transmog Proxy Recovery ' + [Security.Principal.WindowsIdentity]::GetCurrent().User.Value + ' ' + $hash
$service = New-Object -ComObject Schedule.Service
$service.Connect()
$folder = $service.GetFolder('\')
$records = @()
function Invoke-ProductionOperation($operation, $executable) {
    $env:TRANSMOG_INPUT = @{ operation = $operation; journal = $journal; executable = $executable } | ConvertTo-Json -Compress
    & "$env:WINDIR/System32/WindowsPowerShell/v1.0/powershell.exe" -NoLogo -NoProfile -NonInteractive -File $scriptPath
    if ($LASTEXITCODE -ne 0) { throw "Production task operation failed: $operation" }
}
function Get-OwnedProcesses {
    # No unrelated command lines or environment variables are collected.
    @(Get-CimInstance Win32_Process -Filter "Name = 'Transmog recovery test.exe'" | Where-Object { $_.ExecutablePath -ieq $exe } | Select-Object ProcessId, ParentProcessId, SessionId, ExecutablePath, CreationDate)
}
try {
    foreach ($case in @('system-control', 'transmog')) {
        $executable = if ($case -eq 'system-control') { Join-Path $env:WINDIR 'System32/where.exe' } else { $exe }
        Invoke-ProductionOperation 'register' $executable
        $task = $folder.GetTask($name)
        $task.Xml | Set-Content -LiteralPath (Join-Path $out "$case-task.xml") -Encoding UTF8
        $started = [DateTime]::UtcNow
        $running = $task.Run($null)
        $samples = @()
        $lastState = $null
        do {
            Start-Sleep -Milliseconds 100
            try {
                $running.Refresh()
                $state = [int]$running.State
                $active = $state -in @(2, 4)
                $engine = $running.EnginePID
            } catch {
                if ($_.Exception.HResult -ne -2147216629) { throw }
                $state = 'not-running'; $engine = $null; $active = $false
            }
            if ($state -ne $lastState -or ([DateTime]::UtcNow - $started).TotalSeconds -ge 29.8) {
                $samples += [ordered]@{ seconds = ([DateTime]::UtcNow - $started).TotalSeconds; state = $state; enginePid = $engine; lastResult = $task.LastTaskResult; taskState = $task.State; ownedProcesses = @(Get-OwnedProcesses) }
                $lastState = $state
            }
        } while ($active -and ([DateTime]::UtcNow - $started).TotalSeconds -lt 30)
        $records += [ordered]@{ case = $case; activeAtDeadline = $active; samples = $samples; lastResult = $task.LastTaskResult; lastRun = $task.LastRunTime.ToUniversalTime().ToString('o') }
        Save-Json 'scheduled.json' $records
        if ($active) { $task.Stop(0) }
        foreach ($owned in @(Get-OwnedProcesses)) { Stop-Process -Id $owned.ProcessId -Force }
        Invoke-ProductionOperation 'remove' $executable
    }
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $exe
    $start.Arguments = '--recover-proxy "' + $journal + '"'
    $start.UseShellExecute = $false; $start.CreateNoWindow = $true
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.RedirectStandardOutput = $true; $start.RedirectStandardError = $true
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $process = [Diagnostics.Process]::Start($start)
    $processSession = $process.SessionId
    $stdout = $process.StandardOutput.ReadToEndAsync(); $stderr = $process.StandardError.ReadToEndAsync()
    $finished = $process.WaitForExit(30000)
    if (-not $finished) { $process.Kill(); $process.WaitForExit() }
    Save-Json 'direct.json' @{ finished = $finished; exitCode = $process.ExitCode; seconds = $clock.Elapsed.TotalSeconds; sessionId = $processSession }
    $stdout.Result | Set-Content -LiteralPath (Join-Path $out 'direct-stdout.txt')
    $stderr.Result | Set-Content -LiteralPath (Join-Path $out 'direct-stderr.txt')
} finally {
    try { $folder.GetTask($name).Stop(0) } catch { Write-Host $_.Exception.Message }
    foreach ($owned in @(Get-OwnedProcesses)) { Stop-Process -Id $owned.ProcessId -Force }
    Invoke-ProductionOperation 'remove' $exe
    $lease.Dispose()
    $same = (Get-ProxySnapshot) -ceq $before
    Save-Json 'cleanup.json' @{ proxyUnchanged = $same; fixture = $fixture; taskName = $name }
    $resolved = [IO.Path]::GetFullPath($fixture)
    if (-not $resolved.StartsWith($tempRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe fixture path.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
    if (-not $same) { throw 'Unexpected proxy change during no-write diagnosis.' }
}
