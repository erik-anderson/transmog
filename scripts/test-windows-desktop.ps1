param(
    [ValidateRange(0, 240)]
    [int]$SoakMinutes = 0,
    [int]$DevToolsPort = 9333,
    [string]$ScreenshotPath,
    [string]$AutomationScreenshotPath,
    [switch]$SkipReleaseBuild,
    [string]$ExecutablePath,
    [switch]$StartupOnly,
    [switch]$HostedRunnerDevToolsPolicy
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$desktopUi = Join-Path $repositoryRoot 'apps\desktop\ui'
$executable = Join-Path $repositoryRoot 'target\release\transmog.exe'
if ($ExecutablePath) {
    if (-not $SkipReleaseBuild) { throw 'An external executable requires SkipReleaseBuild.' }
    $executable = (Resolve-Path -LiteralPath $ExecutablePath).Path
}
if ($StartupOnly -and $SoakMinutes) { throw 'StartupOnly cannot claim a soak.' }
if ($HostedRunnerDevToolsPolicy -and ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted')) { throw 'Machine debug policy is limited to disposable GitHub-hosted runners.' }
if (Get-Process -Name ([IO.Path]::GetFileNameWithoutExtension($executable)) -ErrorAction SilentlyContinue) { throw 'Close the existing Transmog instance before running isolated desktop validation.' }

function Resolve-ArtifactPath([string]$Path) {
    if (-not $Path) { return $null }
    if ([System.IO.Path]::IsPathRooted($Path)) { return [System.IO.Path]::GetFullPath($Path) }
    return [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $Path))
}

$ScreenshotPath = Resolve-ArtifactPath $ScreenshotPath
$AutomationScreenshotPath = Resolve-ArtifactPath $AutomationScreenshotPath

if (-not $SkipReleaseBuild) { . (Join-Path $PSScriptRoot 'dev-env.ps1') }

if (-not $StartupOnly) {
    Push-Location $desktopUi
    try {
        npm run check
    } finally {
        Pop-Location
    }
}

if (-not $SkipReleaseBuild) {
    cargo build --locked --release -p transmog-desktop
}
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw 'Release desktop executable is unavailable.'
}

$priorArguments = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
$priorLocalAppData = $env:LOCALAPPDATA
$debugArguments = "--remote-debugging-port=$DevToolsPort --remote-debugging-address=127.0.0.1"
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $debugArguments
$probeRoot = Join-Path $repositoryRoot ('artifacts\windows-desktop-validation\' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path (Join-Path $probeRoot 'profile') | Out-Null
$env:LOCALAPPDATA = Join-Path $probeRoot 'profile'
$policyPath = 'HKLM:\SOFTWARE\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments'
$policyName = [IO.Path]::GetFileName($executable)
$priorPolicy = $null
$priorPolicyKind = $null
$createdPolicyKey = $false
$policySet = $false
$process = $null
try {
    $existingTargets = $null
    try { $existingTargets = Invoke-RestMethod -Uri "http://127.0.0.1:$DevToolsPort/json/list" -TimeoutSec 1 } catch { }
    if ($existingTargets) { throw 'The requested DevTools port is already serving another process.' }
    if ($HostedRunnerDevToolsPolicy) {
        if ($env:ACTIONS_ID_TOKEN_REQUEST_TOKEN) { throw 'Desktop execution must not have OIDC permission.' }
        if (-not (Test-Path -LiteralPath $policyPath)) { New-Item -Path $policyPath -Force | Out-Null; $createdPolicyKey = $true }
        $policyKey = Get-Item -LiteralPath $policyPath
        if ($policyKey.GetValueNames() -contains $policyName) { $priorPolicy = $policyKey.GetValue($policyName); $priorPolicyKind = $policyKey.GetValueKind($policyName) }
        New-ItemProperty -LiteralPath $policyPath -Name $policyName -Value $debugArguments -PropertyType String -Force | Out-Null
        $policySet = $true
    }
    $process = Start-Process -FilePath $executable -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $probeRoot 'stdout.txt') -RedirectStandardError (Join-Path $probeRoot 'stderr.txt')
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        try {
            $targets = Invoke-RestMethod -Uri "http://127.0.0.1:$DevToolsPort/json/list" -TimeoutSec 1
        } catch {
            $targets = $null
        }
        if ($targets) { break }
        if ($process.HasExited) { break }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    if (-not $targets) {
        $exited = $process.HasExited
        $exitCode = if ($exited) { $process.ExitCode } else { $null }
        [ordered]@{ StartupVerified = $false; ProcessExited = $exited; ExitCode = $exitCode; HostedPolicy = [bool]$HostedRunnerDevToolsPolicy; Elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) } |
            ConvertTo-Json | Set-Content -LiteralPath (Join-Path $probeRoot 'startup.json') -Encoding utf8NoBOM
        Get-Content -LiteralPath (Join-Path $probeRoot 'stderr.txt') -Tail 30 | ForEach-Object { Write-Host $_ }
        throw "WebView2 DevTools endpoint did not become ready (exited=$exited, exit=$exitCode); startup diagnostics were retained."
    }
    if ($StartupOnly) {
        $result = [pscustomobject]@{ StartupVerified = $true; HostedPolicy = [bool]$HostedRunnerDevToolsPolicy; Runtime = (Invoke-RestMethod -Uri "http://127.0.0.1:$DevToolsPort/json/version").Browser }
        $result | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $probeRoot 'startup.json') -Encoding utf8NoBOM
        return $result
    }

    $startingWorkingSet = (Get-Process -Id $process.Id).WorkingSet64
    Push-Location $desktopUi
    try {
        $smokeArguments = @('run', 'smoke:webview', '--', '--port', "$DevToolsPort", '--soak-minutes', "$SoakMinutes")
        if ($ScreenshotPath) {
            $smokeArguments += @('--screenshot', $ScreenshotPath)
        }
        if ($AutomationScreenshotPath) {
            $smokeArguments += @('--automation-screenshot', $AutomationScreenshotPath)
        }
        & npm @smokeArguments
    } finally {
        Pop-Location
    }
    $endingWorkingSet = (Get-Process -Id $process.Id).WorkingSet64
    if ($endingWorkingSet - $startingWorkingSet -gt 268435456) {
        throw "Desktop working set grew by more than 256 MiB during soak."
    }

    $second = Start-Process -FilePath $executable -PassThru -WindowStyle Hidden
    if (-not $second.WaitForExit(10000)) {
        Stop-Process -Id $second.Id -Force
        throw 'Second desktop process did not hand off to the existing instance.'
    }
    if ($process.HasExited) {
        throw 'Single-instance handoff terminated the primary process.'
    }

    [pscustomobject]@{
        StartupVerified = $true
        AccessibilityVerified = $true
        HighContrastVerified = $true
        HighDpiVerified = $true
        LocalizationLengthVerified = $true
        SoakMinutes = $SoakMinutes
        WorkingSetGrowthBytes = $endingWorkingSet - $startingWorkingSet
        SingleInstanceVerified = $true
    }
} finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $priorArguments
    $env:LOCALAPPDATA = $priorLocalAppData
    if ($policySet) {
        if ($null -ne $priorPolicyKind) { (Get-Item -LiteralPath $policyPath).SetValue($policyName, $priorPolicy, $priorPolicyKind) }
        else { Remove-ItemProperty -LiteralPath $policyPath -Name $policyName -ErrorAction Stop }
    }
    if ($createdPolicyKey) {
        $policyKey = Get-Item -LiteralPath $policyPath
        if ($policyKey.ValueCount -eq 0 -and $policyKey.SubKeyCount -eq 0) { Remove-Item -LiteralPath $policyPath }
    }
}
