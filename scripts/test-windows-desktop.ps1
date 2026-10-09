param(
    [ValidateRange(0, 240)]
    [int]$SoakMinutes = 0,
    [int]$DevToolsPort = 9333,
    [string]$ScreenshotPath,
    [string]$AutomationScreenshotPath,
    [switch]$SkipReleaseBuild,
    [string]$ExecutablePath,
    [switch]$StartupOnly,
    [switch]$ViewerChecks,
    [string]$NativeTracePath,
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
if ($ViewerChecks -and ($StartupOnly -or $SoakMinutes)) { throw 'ViewerChecks is a separate saved-file flow.' }
if ($NativeTracePath -and -not $ViewerChecks) { throw 'NativeTracePath requires ViewerChecks.' }
if ($NativeTracePath) { $NativeTracePath = (Resolve-Path -LiteralPath $NativeTracePath).Path }
if ($HostedRunnerDevToolsPolicy -and ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted')) { throw 'Machine debug policy is limited to disposable GitHub-hosted runners.' }
if (Get-Process -Name ([IO.Path]::GetFileNameWithoutExtension($executable)) -ErrorAction SilentlyContinue) { throw 'Close the existing Transmog instance before running isolated desktop validation.' }

function Resolve-ArtifactPath([string]$Path) {
    if (-not $Path) { return $null }
    if ([System.IO.Path]::IsPathRooted($Path)) { return [System.IO.Path]::GetFullPath($Path) }
    return [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $Path))
}

$ScreenshotPath = Resolve-ArtifactPath $ScreenshotPath
$AutomationScreenshotPath = Resolve-ArtifactPath $AutomationScreenshotPath
foreach ($imagePath in @($ScreenshotPath, $AutomationScreenshotPath)) {
    if ($imagePath) { New-Item -ItemType Directory -Force -Path (Split-Path -Parent $imagePath) | Out-Null }
}

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
$priorTemp = $env:TEMP
$priorTmp = $env:TMP
# Separate captured-page profiles need separate browser debugging endpoints.
$debugPort = if ($ViewerChecks) { 0 } else { $DevToolsPort }
$debugArguments = "--remote-debugging-port=$debugPort --remote-debugging-address=127.0.0.1"
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $debugArguments
$probeRoot = Join-Path $repositoryRoot ('artifacts\windows-desktop-validation\' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path (Join-Path $probeRoot 'profile') | Out-Null
$env:LOCALAPPDATA = Join-Path $probeRoot 'profile'
if ($ViewerChecks) {
    $probeTemp = Join-Path $probeRoot 'temp'
    New-Item -ItemType Directory -Force -Path $probeTemp | Out-Null
    $env:TEMP = $probeTemp
    $env:TMP = $probeTemp
}
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
    $viewerSource = $null
    $pageSource = $null
    if ($ViewerChecks) {
        $viewerSource = Join-Path $probeRoot 'viewer-fixture.saz'
        $responseBody = 'saved viewer response'
        $members = [ordered]@{
            'raw/1_c.txt' = "GET https://example.invalid/path HTTP/1.1`r`nUser-Agent: Transmog fixture`r`n`r`n"
            'raw/1_s.txt' = "HTTP/1.1 200 Fixture`r`nContent-Type: text/plain`r`nContent-Length: $($responseBody.Length)`r`n`r`n$responseBody"
            'raw/1_m.xml' = '<Session BitFlags="1"><SessionTimers ClientBeginRequest="2026-10-08T19:00:00Z" ClientDoneResponse="2026-10-08T19:00:00.025Z"/><SessionFlags><SessionFlag N="x-clientIP" V="192.0.2.25"/></SessionFlags></Session>'
            'transmog/trace-metadata.json' = '{"networkContext":{"command":"ipconfig /all","output":"Fixture IP configuration"}}'
        }
        $zip = [IO.Compression.ZipFile]::Open($viewerSource, [IO.Compression.ZipArchiveMode]::Create)
        try {
            foreach ($member in $members.GetEnumerator()) {
                $entry = $zip.CreateEntry($member.Key)
                $writer = [IO.StreamWriter]::new($entry.Open(), [Text.UTF8Encoding]::new($false))
                try { $writer.Write($member.Value) } finally { $writer.Dispose() }
            }
        } finally { $zip.Dispose() }
        $pageSource = Join-Path $probeRoot 'captured-page-fixture.saz'
        $pageHtml = '<!doctype html><html><head><meta charset="utf-8"><title>Captured fixture</title><link rel="stylesheet" href="/page.css"></head><body><h1 id="captured-heading">Captured page fixture</h1><img id="captured-image" src="/pixel.svg"><script>globalThis.capturedScriptRan=true;Promise.all([fetch("/variant",{method:"POST",headers:{"X-Preview":"dark"},body:"beta"}).then(async r=>({status:r.status,body:await r.text()})),fetch("/variant",{method:"POST",headers:{"X-Preview":"dark"},body:"alpha"}).then(async r=>({status:r.status,body:await r.text()}))]).then(rows=>globalThis.variantResults=rows);fetch("/missing").then(async r=>{globalThis.missingResult={status:r.status,body:await r.text()};});</script></body></html>'
        $pageMembers = [ordered]@{}
        $pageResources = @(
            @{ Id=1; Url='https://example.invalid/captured-page'; Type='text/html'; Body=$pageHtml },
            @{ Id=2; Url='https://example.invalid/page.css'; Type='text/css'; ResponseHeaders="Vary: User-Agent`r`n"; Body='h1 { color: rgb(0, 128, 0); }' },
            @{ Id=4; Url='https://example.invalid/variant'; Method='POST'; RequestBody='alpha'; RequestHeaders="X-Preview: light`r`n"; ResponseHeaders="Vary: X-Preview`r`n"; Type='text/plain'; Body='light variant' },
            @{ Id=5; Url='https://example.invalid/variant'; Method='POST'; RequestBody='beta'; RequestHeaders="X-Preview: dark`r`n"; ResponseHeaders="Vary: X-Preview`r`n"; Type='text/plain'; Body='dark variant' },
            @{ Id=3; Url='https://example.invalid/pixel.svg'; Type='image/svg+xml'; Body='<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20" fill="green"/></svg>' }
        )
        foreach ($resource in $pageResources) {
            $number = $resource.Id
            $method = if ($resource.Method) { $resource.Method } else { 'GET' }
            $requestBody = if ($resource.RequestBody) { $resource.RequestBody } else { '' }
            $requestLength = [Text.Encoding]::UTF8.GetByteCount($requestBody)
            $pageMembers["raw/${number}_c.txt"] = "$method $($resource.Url) HTTP/1.1`r`nHost: example.invalid`r`nUser-Agent: Placeholder/1`r`n$($resource.RequestHeaders)Content-Length: $requestLength`r`n`r`n$requestBody"
            $length = [Text.Encoding]::UTF8.GetByteCount($resource.Body)
            $pageMembers["raw/${number}_s.txt"] = "HTTP/1.1 200 OK`r`nContent-Type: $($resource.Type)`r`n$($resource.ResponseHeaders)Content-Length: $length`r`n`r`n$($resource.Body)"
            $pageMembers["raw/${number}_m.xml"] = '<Session><SessionTimers ClientBeginRequest="2026-10-08T19:00:00Z" ClientDoneResponse="2026-10-08T19:00:00.025Z"/></Session>'
        }
        $zip = [IO.Compression.ZipFile]::Open($pageSource, [IO.Compression.ZipArchiveMode]::Create)
        try {
            foreach ($member in $pageMembers.GetEnumerator()) {
                $entry = $zip.CreateEntry($member.Key)
                $writer = [IO.StreamWriter]::new($entry.Open(), [Text.UTF8Encoding]::new($false))
                try { $writer.Write($member.Value) } finally { $writer.Dispose() }
            }
        } finally { $zip.Dispose() }
    }
    $launchArguments = @{ FilePath = $executable; PassThru = $true; WindowStyle = 'Hidden'; RedirectStandardOutput = (Join-Path $probeRoot 'stdout.txt'); RedirectStandardError = (Join-Path $probeRoot 'stderr.txt') }
    if ($viewerSource) { $launchArguments.ArgumentList = ('"' + $viewerSource + '"') }
    $process = Start-Process @launchArguments
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        if ($ViewerChecks) {
            $endpoint = Get-ChildItem -LiteralPath $probeRoot -Filter DevToolsActivePort -Recurse -File | Select-Object -First 1
            if ($endpoint) { $DevToolsPort = [int](Get-Content -LiteralPath $endpoint.FullName -First 1) }
        }
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
    $invalidTrace = Join-Path $probeRoot 'invalid.tmcap'
    [IO.File]::WriteAllText($invalidTrace, 'Invalid capture fixture')
    Push-Location $desktopUi
    try {
        $smokeArguments = @('run', 'smoke:webview', '--', '--port', "$DevToolsPort", '--soak-minutes', "$SoakMinutes", '--invalid-trace', $invalidTrace)
        if ($ScreenshotPath) {
            $smokeArguments += @('--screenshot', $ScreenshotPath)
        }
        if ($AutomationScreenshotPath) {
            $smokeArguments += @('--automation-screenshot', $AutomationScreenshotPath)
        }
        if ($ViewerChecks) {
            $viewerArguments = @('scripts/smoke-viewers.mjs', '--port', "$DevToolsPort", '--source', $viewerSource, '--executable', $executable)
            $viewerArguments += @('--page-source', $pageSource)
            $viewerArguments += @('--profile-root', $probeRoot)
            $viewerArguments += @('--process-id', "$($process.Id)", '--close-helper', (Join-Path $PSScriptRoot 'close-desktop-probe-window.ps1'))
            if ($ScreenshotPath) { $viewerArguments += @('--screenshot', $ScreenshotPath) }
            if ($NativeTracePath) { $viewerArguments += @('--native-source', $NativeTracePath) }
            & node @viewerArguments
        } else { & npm @smokeArguments }
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
        AccessibilityVerified = -not $ViewerChecks
        HighContrastVerified = -not $ViewerChecks
        HighDpiVerified = -not $ViewerChecks
        LocalizationLengthVerified = -not $ViewerChecks
        ViewerFlowVerified = [bool]$ViewerChecks
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
    $env:TEMP = $priorTemp
    $env:TMP = $priorTmp
    if ($policySet) {
        if ($null -ne $priorPolicyKind) { (Get-Item -LiteralPath $policyPath).SetValue($policyName, $priorPolicy, $priorPolicyKind) }
        else { Remove-ItemProperty -LiteralPath $policyPath -Name $policyName -ErrorAction Stop }
    }
    if ($createdPolicyKey) {
        $policyKey = Get-Item -LiteralPath $policyPath
        if ($policyKey.ValueCount -eq 0 -and $policyKey.SubKeyCount -eq 0) { Remove-Item -LiteralPath $policyPath }
    }
}
