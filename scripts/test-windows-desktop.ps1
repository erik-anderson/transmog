param(
    [ValidateRange(0, 240)]
    [int]$SoakMinutes = 0,
    [int]$DevToolsPort = 9333,
    [string]$ScreenshotPath,
    [switch]$SkipReleaseBuild
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$desktopUi = Join-Path $repositoryRoot 'apps\desktop\ui'
$executable = Join-Path $repositoryRoot 'target\release\transmog-desktop.exe'

. (Join-Path $PSScriptRoot 'dev-env.ps1')

Push-Location $desktopUi
try {
    npm run check
} finally {
    Pop-Location
}

if (-not $SkipReleaseBuild) {
    cargo build --locked --release -p transmog-desktop
}
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw 'Release desktop executable is unavailable.'
}

$priorArguments = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$DevToolsPort"
$process = $null
try {
    $process = Start-Process -FilePath $executable -PassThru -WindowStyle Hidden
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        try {
            $targets = Invoke-RestMethod -Uri "http://127.0.0.1:$DevToolsPort/json/list" -TimeoutSec 1
        } catch {
            $targets = $null
        }
        if ($targets) { break }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    if (-not $targets) { throw 'WebView2 DevTools endpoint did not become ready.' }

    $startingWorkingSet = (Get-Process -Id $process.Id).WorkingSet64
    Push-Location $desktopUi
    try {
        $smokeArguments = @('run', 'smoke:webview', '--', '--port', "$DevToolsPort", '--soak-minutes', "$SoakMinutes")
        if ($ScreenshotPath) {
            $smokeArguments += @('--screenshot', $ScreenshotPath)
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
}
