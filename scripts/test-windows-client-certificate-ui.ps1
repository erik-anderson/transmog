#requires -Version 7
param([switch]$SkipBuild, [int]$DevToolsPort = 9334, [switch]$HostedRunnerDevToolsPolicy)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
if ($HostedRunnerDevToolsPolicy -and ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted')) { throw 'Machine debug policy is limited to disposable GitHub-hosted runners.' }
if ($HostedRunnerDevToolsPolicy -and $env:ACTIONS_ID_TOKEN_REQUEST_TOKEN) { throw 'Desktop execution must not have OIDC permission.' }
$repositoryRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'dev-env.ps1')
if (Get-Process -Name transmog -ErrorAction SilentlyContinue) { throw 'Close Transmog before running isolated certificate acceptance.' }
$existingTargets = $null
try { $existingTargets = Invoke-RestMethod -Uri "http://127.0.0.1:$DevToolsPort/json/list" -TimeoutSec 1 } catch { }
if ($existingTargets) { throw 'The requested DevTools port already belongs to another process.' }
if (-not $SkipBuild) { cargo build --locked -p transmog-desktop --features desktop-test-fixtures }
$executable = Join-Path $repositoryRoot 'target\debug\transmog.exe'
$probeRoot = Join-Path $repositoryRoot ('artifacts\client-certificate-desktop\' + [Guid]::NewGuid().ToString('N'))
$profile = Join-Path $probeRoot 'profile'
New-Item -ItemType Directory -Force -Path $profile | Out-Null
$priorLocalData = $env:LOCALAPPDATA
$priorArguments = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
$priorFixture = $env:TRANSMOG_DESKTOP_CERTIFICATE_TEST
$debugArguments = "--remote-debugging-port=$DevToolsPort --remote-debugging-address=127.0.0.1"
$policyPath = 'HKLM:\SOFTWARE\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments'
$policyName = [IO.Path]::GetFileName($executable)
$priorPolicy = $null
$priorPolicyKind = $null
$createdPolicyKey = $false
$policySet = $false
$process = $null
try {
    $env:LOCALAPPDATA = $profile
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $debugArguments
    $env:TRANSMOG_DESKTOP_CERTIFICATE_TEST = '1'
    # Elevated hosted WebView2 ignores environment flags; use the same scoped
    # machine policy as the release desktop gate, then restore it in finally.
    if ($HostedRunnerDevToolsPolicy) {
        if (-not (Test-Path -LiteralPath $policyPath)) { New-Item -Path $policyPath -Force | Out-Null; $createdPolicyKey = $true }
        $policyKey = Get-Item -LiteralPath $policyPath
        if ($policyKey.GetValueNames() -contains $policyName) { $priorPolicy = $policyKey.GetValue($policyName, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames); $priorPolicyKind = $policyKey.GetValueKind($policyName) }
        New-ItemProperty -LiteralPath $policyPath -Name $policyName -Value $debugArguments -PropertyType String -Force | Out-Null
        $policySet = $true
    }
    foreach ($phase in @('full','restart')) {
        $phaseRoot = Join-Path $probeRoot $phase
        New-Item -ItemType Directory -Force -Path $phaseRoot | Out-Null
        $process = Start-Process -FilePath $executable -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $phaseRoot 'stdout.txt') -RedirectStandardError (Join-Path $phaseRoot 'stderr.txt')
        try {
            $ready = $false
            for ($attempt = 0; $attempt -lt 150; $attempt++) {
                if ($process.HasExited) { throw "Certificate acceptance process exited: $($process.ExitCode)" }
                try { $targets = Invoke-RestMethod -Uri "http://127.0.0.1:$DevToolsPort/json/list" -TimeoutSec 1; if ($targets | Where-Object { $_.type -eq 'page' -and $_.title -eq 'Transmog' }) { $ready = $true; break } } catch { }
                Start-Sleep -Milliseconds 200
            }
            if (-not $ready) { throw 'The isolated WebView2 debugging endpoint did not start.' }
            & node (Join-Path $repositoryRoot 'apps\desktop\ui\scripts\check-native-client-certificates.mjs') --port $DevToolsPort --mode $phase --output $probeRoot
        } finally {
            if (-not $process.HasExited) {
                [void]$process.CloseMainWindow()
                if (-not $process.WaitForExit(15000)) { Stop-Process -Id $process.Id; $process.WaitForExit() }
            }
            $process = $null
            $record = Join-Path $profile 'Transmog\certificate-test-fixture.json'
            if (Test-Path -LiteralPath $record) {
                $owned = Get-Content -LiteralPath $record -Raw | ConvertFrom-Json
                & (Join-Path $PSScriptRoot 'client-certificate-test-fixture.ps1') -Action Cleanup -Thumbprint $owned.thumbprint -IssuerThumbprint $owned.issuerThumbprint
            }
        }
    }
    Write-Output "Certificate desktop acceptance passed. Evidence: $probeRoot"
} finally {
    $env:LOCALAPPDATA = $priorLocalData
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $priorArguments
    $env:TRANSMOG_DESKTOP_CERTIFICATE_TEST = $priorFixture
    if ($policySet) {
        if ($null -ne $priorPolicyKind) { (Get-Item -LiteralPath $policyPath).SetValue($policyName, $priorPolicy, $priorPolicyKind) }
        else { Remove-ItemProperty -LiteralPath $policyPath -Name $policyName -ErrorAction Stop }
    }
    if ($createdPolicyKey) {
        $policyKey = Get-Item -LiteralPath $policyPath
        if ($policyKey.ValueCount -eq 0 -and $policyKey.SubKeyCount -eq 0) { Remove-Item -LiteralPath $policyPath }
    }
}
