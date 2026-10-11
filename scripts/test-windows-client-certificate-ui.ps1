#requires -Version 7
param([switch]$SkipBuild, [int]$DevToolsPort = 9334)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
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
$process = $null
try {
    $env:LOCALAPPDATA = $profile
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$DevToolsPort --remote-debugging-address=127.0.0.1"
    $env:TRANSMOG_DESKTOP_CERTIFICATE_TEST = '1'
    foreach ($phase in @('full','restart')) {
        $process = Start-Process -FilePath $executable -WindowStyle Hidden -PassThru
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
}
