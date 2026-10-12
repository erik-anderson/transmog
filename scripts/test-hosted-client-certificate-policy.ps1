# Run the real certificate wrapper with process/network/registry boundaries
# mocked. Never change this workstation's WebView policy or launch Transmog.
param()
$ErrorActionPreference = 'Stop'
$priorNativePreference = $PSNativeCommandUseErrorActionPreference
$priorExitCode = $global:LASTEXITCODE
$environmentBefore = [Environment]::GetEnvironmentVariables()
$fixtureParent = [IO.Path]::GetTempPath()
$fixtureName = 'transmog-hosted-certificate-test-' + [Guid]::NewGuid().ToString('N')
$fixtureRoot = Join-Path $fixtureParent $fixtureName
$fixturePolicyPath = 'HKLM:\SOFTWARE\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments'
New-Item -ItemType Directory -Path (Join-Path $fixtureRoot 'scripts') -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'test-windows-client-certificate-ui.ps1') -Destination (Join-Path $fixtureRoot 'scripts')
# Skip native environment discovery for these boundary tests.
'' | Set-Content -LiteralPath (Join-Path $fixtureRoot 'scripts/dev-env.ps1')

function Test-Path {
    param([string]$LiteralPath)
    if ($LiteralPath -ceq $fixturePolicyPath) { return $fixtureState.KeyExists }
    Microsoft.PowerShell.Management\Test-Path -LiteralPath $LiteralPath
}
function New-Item {
    param([string]$Path, [string]$ItemType, [switch]$Force)
    if ($Path -ceq $fixturePolicyPath) { $fixtureState.KeyExists = $true; $fixtureState.Writes++; return }
    Microsoft.PowerShell.Management\New-Item -Path $Path -ItemType $ItemType -Force:$Force
}
function Get-Item {
    param([string]$LiteralPath)
    if ($LiteralPath -cne $fixturePolicyPath) { throw 'Unexpected registry read.' }
    $key = [pscustomobject]@{ Values = $fixtureState.Values; Kinds = $fixtureState.Kinds; SubKeyCount = 0 }
    $key | Add-Member -MemberType ScriptProperty -Name ValueCount -Value { $this.Values.Count }
    $key | Add-Member -MemberType ScriptMethod -Name GetValueNames -Value { @($this.Values.Keys) }
    $key | Add-Member -MemberType ScriptMethod -Name GetValue -Value {
        param($name, $defaultValue, $options)
        if ($options -ne [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) { throw 'Prior policy must be read without expanding environment variables.' }
        $this.Values[$name]
    }
    $key | Add-Member -MemberType ScriptMethod -Name GetValueKind -Value { param($name) $this.Kinds[$name] }
    $key | Add-Member -MemberType ScriptMethod -Name SetValue -Value { param($name, $value, $kind) $this.Values[$name] = $value; $this.Kinds[$name] = $kind }
    $key
}
function New-ItemProperty {
    param([string]$LiteralPath, [string]$Name, [string]$Value, [string]$PropertyType, [switch]$Force)
    if ($LiteralPath -cne $fixturePolicyPath -or $Name -cne 'transmog.exe') { throw 'Policy write was not scoped to Transmog.' }
    $fixtureState.Values[$Name] = $Value
    $fixtureState.Kinds[$Name] = $PropertyType
    $fixtureState.Writes++
}
function Remove-ItemProperty {
    param([string]$LiteralPath, [string]$Name, [string]$ErrorAction)
    if ($LiteralPath -cne $fixturePolicyPath -or $Name -cne 'transmog.exe') { throw 'Unexpected registry value removal.' }
    $fixtureState.Values.Remove($Name)
    $fixtureState.Kinds.Remove($Name)
}
function Remove-Item {
    param([string]$LiteralPath)
    if ($LiteralPath -cne $fixturePolicyPath -or $fixtureState.Values.Count) { throw 'Unexpected registry key removal.' }
    $fixtureState.KeyExists = $false
}
function Get-Process { param([string]$Name, [string]$ErrorAction) }
function Invoke-RestMethod {
    param([string]$Uri, [int]$TimeoutSec)
    if ($Uri -cne 'http://127.0.0.1:9334/json/list') { throw 'Debug endpoint was not loopback scoped.' }
    if ($fixtureState.Running -and $fixtureState.Failure -ne 'endpoint') { [pscustomobject]@{ type = 'page'; title = 'Transmog' } }
}
function Start-Sleep { param([int]$Milliseconds) }
function Start-Process {
    param([string]$FilePath, [string]$WindowStyle, [switch]$PassThru, [string]$RedirectStandardOutput, [string]$RedirectStandardError)
    if ($WindowStyle -cne 'Hidden' -or [IO.Path]::GetFileName($FilePath) -cne 'transmog.exe') { throw 'Unexpected desktop launch.' }
    $fixtureState.Launches++
    if ($fixtureState.Failure -eq 'launch') { throw 'Injected process launch failure.' }
    if ([IO.Path]::GetFileName($RedirectStandardOutput) -cne 'stdout.txt' -or [IO.Path]::GetFileName($RedirectStandardError) -cne 'stderr.txt') { throw 'Missing startup diagnostics.' }
    'fixture stdout' | Set-Content -LiteralPath $RedirectStandardOutput
    'fixture stderr' | Set-Content -LiteralPath $RedirectStandardError
    $fixtureState.Running = $true
    $process = [pscustomobject]@{ HasExited = $false; State = $fixtureState }
    $process | Add-Member -MemberType ScriptMethod -Name CloseMainWindow -Value { $this.State.Stops++; $true }
    $process | Add-Member -MemberType ScriptMethod -Name WaitForExit -Value { param($timeout) $this.HasExited = $true; $this.State.Running = $false; $true }
    $process
}
function node {
    if ($env:TRANSMOG_DESKTOP_CERTIFICATE_TEST -cne '1') { throw 'Certificate fixture was not enabled.' }
    if ($env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -cne '--remote-debugging-port=9334 --remote-debugging-address=127.0.0.1') { throw 'Unexpected browser arguments.' }
    if ($fixtureState.Hosted -and ($fixtureState.Values['transmog.exe'] -cne $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -or $fixtureState.Kinds['transmog.exe'] -cne 'String')) { throw 'Hosted policy was not active during the test.' }
    $fixtureState.NodeCalls++
    if ($fixtureState.Failure -eq 'node') { throw 'Injected certificate check failure.' }
}

try {
    $global:LASTEXITCODE = 0
    foreach ($case in @(
        @{ Name = 'local environment flags'; Hosted = $false; Key = $true; Value = $true },
        @{ Name = 'reject a non-hosted runner'; Hosted = $true; Runner = 'self-hosted'; Error = 'Machine debug policy is limited'; Launches = 0 },
        @{ Name = 'reject an OIDC-capable job'; Hosted = $true; Oidc = 'fixture'; Error = 'Desktop execution must not have OIDC'; Launches = 0 },
        @{ Name = 'remove a newly created empty key'; Hosted = $true },
        @{ Name = 'preserve unrelated policy values'; Hosted = $true; Key = $true },
        @{ Name = 'restore the prior value and registry type'; Hosted = $true; Key = $true; Value = $true },
        @{ Name = 'restore policy after certificate checks fail'; Hosted = $true; Key = $true; Value = $true; Failure = 'node'; Error = 'Injected certificate check failure'; Launches = 1 },
        @{ Name = 'restore policy after endpoint timeout'; Hosted = $true; Failure = 'endpoint'; Error = 'debugging endpoint did not start'; Launches = 1 },
        @{ Name = 'restore policy after process launch fails'; Hosted = $true; Key = $true; Value = $true; Failure = 'launch'; Error = 'Injected process launch failure'; Launches = 1 }
    )) {
        $fixtureState = [pscustomobject]@{ KeyExists = [bool]$case.Key; Values = @{}; Kinds = @{}; Writes = 0; Running = $false; Launches = 0; Stops = 0; NodeCalls = 0; Hosted = [bool]$case.Hosted; Failure = $case.Failure }
        if ($case.Key) { $fixtureState.Values['other.exe'] = 'unrelated'; $fixtureState.Kinds['other.exe'] = 'String' }
        if ($case.Value) { $fixtureState.Values['transmog.exe'] = '%prior-arguments%'; $fixtureState.Kinds['transmog.exe'] = 'ExpandString' }
        $env:GITHUB_ACTIONS = 'true'
        $env:RUNNER_ENVIRONMENT = if ($case.Runner) { $case.Runner } else { 'github-hosted' }
        $env:ACTIONS_ID_TOKEN_REQUEST_TOKEN = $case.Oidc
        $beforeRun = [Environment]::GetEnvironmentVariables()
        $caught = $null
        try { & (Join-Path $fixtureRoot 'scripts/test-windows-client-certificate-ui.ps1') -SkipBuild -HostedRunnerDevToolsPolicy:$case.Hosted | Out-Null } catch { $caught = $_.Exception.Message }
        if ($case.Error) { if (-not $caught -or -not $caught.Contains($case.Error)) { throw "Wrong failure in $($case.Name): $caught" } }
        elseif ($caught) { throw "Unexpected failure in $($case.Name): $caught" }
        $expectedLaunches = if ($case.ContainsKey('Launches')) { $case.Launches } else { 2 }
        if ($fixtureState.Launches -ne $expectedLaunches -or $fixtureState.Running) { throw "Process lifecycle failed in $($case.Name)." }
        if ($case.Failure -ne 'launch' -and $fixtureState.Stops -ne $expectedLaunches) { throw "Owned process cleanup failed in $($case.Name)." }
        if (-not $case.Error -and $fixtureState.NodeCalls -ne 2) { throw 'Both full and restart phases must run.' }
        if ($expectedLaunches -eq 0 -and $fixtureState.Writes) { throw 'Rejected mode wrote registry policy.' }
        if ($fixtureState.KeyExists -ne [bool]$case.Key) { throw "Policy key was not restored in $($case.Name)." }
        $expectedValues = [int][bool]$case.Key + [int][bool]$case.Value
        if ($fixtureState.Values.Count -ne $expectedValues) { throw "Policy values were not restored in $($case.Name)." }
        if ($case.Key -and $fixtureState.Values['other.exe'] -cne 'unrelated') { throw 'Unrelated policy was changed.' }
        if ($case.Value -and ($fixtureState.Values['transmog.exe'] -cne '%prior-arguments%' -or $fixtureState.Kinds['transmog.exe'] -cne 'ExpandString')) { throw 'Prior policy value or type was changed.' }
        $afterRun = [Environment]::GetEnvironmentVariables()
        foreach ($name in @(@($beforeRun.Keys) + @($afterRun.Keys) | Select-Object -Unique)) {
            if ($beforeRun.Contains($name) -ne $afterRun.Contains($name) -or $beforeRun[$name] -cne $afterRun[$name]) { throw "Caller environment changed: $name." }
        }
        Write-Host "Passed $($case.Name)."
    }
} finally {
    $environmentAfter = [Environment]::GetEnvironmentVariables()
    foreach ($name in @(@($environmentBefore.Keys) + @($environmentAfter.Keys) | Select-Object -Unique)) {
        if ($environmentBefore.Contains($name)) { [Environment]::SetEnvironmentVariable($name, $environmentBefore[$name], 'Process') }
        else { Microsoft.PowerShell.Management\Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
    }
    $PSNativeCommandUseErrorActionPreference = $priorNativePreference
    $global:LASTEXITCODE = $priorExitCode
    $resolvedFixture = (Resolve-Path -LiteralPath $fixtureRoot).Path
    $resolvedParent = (Resolve-Path -LiteralPath $fixtureParent).Path
    if ([IO.Path]::GetRelativePath($resolvedParent, $resolvedFixture) -cne $fixtureName) { throw 'Refusing cleanup outside the temporary fixture owner.' }
    Microsoft.PowerShell.Management\Remove-Item -LiteralPath $resolvedFixture -Recurse -Force
}
