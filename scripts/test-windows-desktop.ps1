param(
    [ValidateRange(0, 240)]
    [int]$SoakMinutes = 0,
    [int]$DevToolsPort = 9333,
    [string]$ScreenshotPath,
    [string]$AutomationScreenshotPath,
    [switch]$SkipReleaseBuild,
    [string]$ExecutablePath,
    [switch]$StartupOnly,
    [switch]$FormatConverterChecks,
    [switch]$WindowGeometryChecks,
    [switch]$PortableCaseChecks,
    [switch]$ViewerChecks,
    [switch]$CapturedPageChecks,
    [switch]$TraceSaveChecks,
    [switch]$KeyRecoveryChecks,
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
if ($FormatConverterChecks -and ($StartupOnly -or $ViewerChecks -or $PortableCaseChecks -or $WindowGeometryChecks -or $KeyRecoveryChecks -or $SoakMinutes)) { throw 'FormatConverterChecks is an isolated converter-window flow.' }
if ($WindowGeometryChecks -and ($StartupOnly -or $ViewerChecks -or $PortableCaseChecks -or $KeyRecoveryChecks -or $SoakMinutes)) { throw 'WindowGeometryChecks is an isolated close/reopen flow.' }
if ($PortableCaseChecks -and ($StartupOnly -or $ViewerChecks -or $SoakMinutes)) { throw 'PortableCaseChecks is an isolated main-window flow.' }
if ($PortableCaseChecks -and -not $ExecutablePath) { throw 'PortableCaseChecks requires an explicit desktop-test-fixtures build via ExecutablePath and SkipReleaseBuild.' }
if ($ViewerChecks -and ($StartupOnly -or $SoakMinutes)) { throw 'ViewerChecks is a separate saved-file flow.' }
if ($CapturedPageChecks -and -not $ViewerChecks) { throw 'CapturedPageChecks requires ViewerChecks.' }
if ($TraceSaveChecks -and (-not $ViewerChecks -or $NativeTracePath)) { throw 'TraceSaveChecks requires ViewerChecks and uses its isolated SAZ fixture.' }
if ($KeyRecoveryChecks -and ($StartupOnly -or $ViewerChecks)) { throw 'KeyRecoveryChecks requires the main-window smoke checks.' }
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
if ($WindowGeometryChecks) {
    & (Join-Path $PSScriptRoot 'test-window-geometry.ps1') -ExecutablePath $executable
    return
}

$priorArguments = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
$priorLocalAppData = $env:LOCALAPPDATA
$priorTemp = $env:TEMP
$priorTmp = $env:TMP
$priorCaseTest = $env:TRANSMOG_DESKTOP_CASE_TEST
if ($PortableCaseChecks) { $env:TRANSMOG_DESKTOP_CASE_TEST = '1' }
# Separate captured-page profiles need separate browser debugging endpoints.
$debugPort = if ($ViewerChecks -or $PortableCaseChecks) { 0 } else { $DevToolsPort }
$debugArguments = "--remote-debugging-port=$debugPort --remote-debugging-address=127.0.0.1"
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $debugArguments
$probeRoot = Join-Path $repositoryRoot ('artifacts\windows-desktop-validation\' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path (Join-Path $probeRoot 'profile') | Out-Null
$env:LOCALAPPDATA = Join-Path $probeRoot 'profile'
$extensionSource = $null
$visualizerTrace = $null
if (-not $ViewerChecks -and -not $PortableCaseChecks -and -not $StartupOnly) {
    $extensionSource = Join-Path $probeRoot 'extension-source'
    $extensionRegistry = Join-Path $env:LOCALAPPDATA 'Transmog/extensions-v1'
    New-Item -ItemType Directory -Force -Path $extensionSource, $extensionRegistry | Out-Null
    $extensionScript = "throw new Error('Management must not evaluate extension code'); export function visualize() {}"
    [IO.File]::WriteAllText((Join-Path $extensionSource 'main.js'), $extensionScript)
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'fixtures/visualizer.js') -Destination (Join-Path $extensionSource 'parser.js')
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'fixtures/visualizer-context.js') -Destination (Join-Path $extensionSource 'context.js')
    $extensionManifest = @{formatVersion=1;apiVersion=1;runtime='javascript-es2022';id='test.development';version='1.0.0';name='Development visualizer fixture';files=@(@{path='main.js';bytes=0});visualizers=@(@{id='headers';name='Example headers';entry='main.js';surfaces=@('headers');inputs=@{request=@{headers=@('x-example');body='none'};response=@{headers=@();body='none'}};matches=@(@{direction='request';headers=@(@{direction='request';name='x-example';test=@{op='present'}})})})}
    $extensionManifest.files += @{path='parser.js';bytes=0}
    $extensionManifest.files += @{path='context.js';bytes=0}
    $nativeInputs = @{request=@{headers=@('x-example');body='none'};response=@{headers=@('content-type');body='none'}}
    $nativeMatches = @(@{direction='request';headers=@(@{direction='request';name='x-example';test=@{op='present'}})},@{direction='response';contentTypes=@('text/plain')})
    $extensionManifest.visualizers += @{id='native-headers';name='Native fields';entry='parser.js';surfaces=@('headers');inputs=$nativeInputs;matches=$nativeMatches}
    $extensionManifest.visualizers += @{id='native-body';name='Native protocol';entry='parser.js';surfaces=@('body');inputs=@{request=@{headers=@();body='none'};response=@{headers=@('content-type');body='required'}};matches=@(@{direction='response';contentTypes=@('text/plain')})}
    $extensionManifest.visualizers += @{id='native-context';name='Native context';entry='context.js';surfaces=@('body');inputs=@{request=@{headers=@('x-example');body='none'};response=@{headers=@('content-type');body='optional'}};matches=$nativeMatches;context=@{headers=@(@{direction='request';name='x-example';test=@{op='present'}});maxExchanges=8;lookAhead=$true}}
    $visualizerTrace = Join-Path $probeRoot 'visualizer-fixture.saz'
    $fixtureBody = 'Synthetic body evidence'
    $fixtureMembers = [ordered]@{'raw/1_c.txt'="GET https://extension.invalid/extension-fixture HTTP/1.1`r`nX-Example: Native header evidence`r`n`r`n";'raw/1_s.txt'="HTTP/1.1 200 OK`r`nContent-Type: text/plain`r`nContent-Length: $($fixtureBody.Length)`r`n`r`n$fixtureBody"}
    $fixtureTargets = @('https://extension.invalid/extension-fixture','https://other.invalid/extension-fixture','https://extension.invalid:8443/extension-fixture','http://extension.invalid:443/extension-fixture')
    for ($fixtureIndex = 0; $fixtureIndex -lt $fixtureTargets.Length; $fixtureIndex++) {
        $fixtureNumber = $fixtureIndex + 2
        $fixtureByte = if ($fixtureIndex -eq 0) { 'A' } else { 'B' }
        $fixtureMembers["raw/${fixtureNumber}_c.txt"] = "GET $($fixtureTargets[$fixtureIndex]) HTTP/1.1`r`nX-Example: Native header evidence`r`n`r`n"
        $fixtureMembers["raw/${fixtureNumber}_s.txt"] = "HTTP/1.1 200 OK`r`nContent-Type: text/plain`r`nContent-Length: 1`r`n`r`n$fixtureByte"
    }
    for ($fixtureNumber = 1; $fixtureNumber -le 5; $fixtureNumber++) {
        $fixtureMembers["raw/${fixtureNumber}_m.xml"] = "<Session SID=`"$fixtureNumber`"><SessionFlags><SessionFlag N=`"x-processinfo`" V=`"synthetic.exe:42`" /></SessionFlags></Session>"
    }
    $otherMembers = [ordered]@{'raw/1_c.txt'=$fixtureMembers['raw/1_c.txt'];'raw/1_s.txt'="HTTP/1.1 200 OK`r`nContent-Type: text/plain`r`nContent-Length: 1`r`n`r`nB";'raw/1_m.xml'=$fixtureMembers['raw/1_m.xml']}
    $fixtureArchives = @(@{path=$visualizerTrace;members=$fixtureMembers},@{path=($visualizerTrace -replace '\.saz$','-other.saz');members=$otherMembers})
    foreach ($fixtureArchive in $fixtureArchives) {
        $fixtureZip = [IO.Compression.ZipFile]::Open($fixtureArchive.path, [IO.Compression.ZipArchiveMode]::Create)
        try {
            foreach ($member in $fixtureArchive.members.GetEnumerator()) {
                $writer = [IO.StreamWriter]::new($fixtureZip.CreateEntry($member.Key).Open(), [Text.UTF8Encoding]::new($false))
                try { $writer.Write($member.Value) } finally { $writer.Dispose() }
            }
        } finally { $fixtureZip.Dispose() }
    }
    $extensionManifestJson = $extensionManifest | ConvertTo-Json -Depth 16 -Compress
    [IO.File]::WriteAllText((Join-Path $extensionSource 'manifest.json'), $extensionManifestJson)
    $extensionDigest = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($extensionManifestJson))).ToLowerInvariant()
    $extensionEntry = @{catalog=@{manifest=$extensionManifest;digest=$extensionDigest;enabled=$true;origin='development'};directory=$extensionSource;hotReload=$false}
    [IO.File]::WriteAllText((Join-Path $extensionRegistry 'catalog.json'), (ConvertTo-Json -InputObject @($extensionEntry) -Depth 20 -Compress))
}
if ($ViewerChecks -or $PortableCaseChecks) {
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
    if ($KeyRecoveryChecks) {
        # No trust-store changes: seed an owned fixture whose DPAPI blob is corrupt.
        $fixtureDirectory = Join-Path $env:LOCALAPPDATA 'Transmog'
        New-Item -ItemType Directory -Force -Path $fixtureDirectory | Out-Null
        $fixtureKey = [Security.Cryptography.ECDsa]::Create([Security.Cryptography.ECCurve+NamedCurves]::nistP256)
        try {
            $fixtureRequest = [Security.Cryptography.X509Certificates.CertificateRequest]::new('CN=Transmog DPAPI recovery fixture', $fixtureKey, [Security.Cryptography.HashAlgorithmName]::SHA256)
            $fixtureCertificate = $fixtureRequest.CreateSelfSigned([DateTimeOffset]::UtcNow.AddDays(-1), [DateTimeOffset]::UtcNow.AddDays(2))
            try {
                [IO.File]::WriteAllText((Join-Path $fixtureDirectory 'interception-ca.pem'), $fixtureCertificate.ExportCertificatePem())
                $fixtureHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($fixtureCertificate.RawData))
                [IO.File]::WriteAllText((Join-Path $fixtureDirectory 'certificate-ownership-v1.json'), (@{schema=1;sha256=$fixtureHash} | ConvertTo-Json -Compress))
                Add-Type -AssemblyName System.Security.Cryptography.ProtectedData
                $fixturePlaintext = [Text.Encoding]::UTF8.GetBytes($fixtureKey.ExportPkcs8PrivateKeyPem())
                $fixtureEntropy = [Text.Encoding]::UTF8.GetBytes('Transmog interception CA private key v1')
                $fixtureProtected = [Security.Cryptography.ProtectedData]::Protect($fixturePlaintext, $fixtureEntropy, [Security.Cryptography.DataProtectionScope]::CurrentUser)
                [Array]::Clear($fixturePlaintext)
                $fixtureProtected[-1] = $fixtureProtected[-1] -bxor 1
                $fixtureBytes = [byte[]]([Text.Encoding]::UTF8.GetBytes("TRANSMOG-CA-KEY`0") + [byte[]]@(1,1) + $fixtureProtected)
                $fixturePath = Join-Path $fixtureDirectory 'interception-ca.key'
                [IO.File]::WriteAllBytes($fixturePath, $fixtureBytes)
            } finally { $fixtureCertificate.Dispose() }
        } finally { $fixtureKey.Dispose() }
    }
    if ($ViewerChecks) {
        $viewerSource = Join-Path $probeRoot 'viewer-fixture.saz'
        $trafficSource = Join-Path $probeRoot 'traffic-fixture.saz'
        $trafficZip = [IO.Compression.ZipFile]::Open($trafficSource, [IO.Compression.ZipArchiveMode]::Create)
        try {
            for ($trafficIndex = 1; $trafficIndex -le 20000; $trafficIndex++) {
                foreach ($trafficSide in @('c','s')) {
                    $trafficEntry = $trafficZip.CreateEntry("raw/${trafficIndex}_${trafficSide}.txt")
                    $trafficWriter = [IO.StreamWriter]::new($trafficEntry.Open(), [Text.UTF8Encoding]::new($false))
                    try {
                        if ($trafficSide -eq 'c') { $trafficWriter.Write("GET https://example.invalid/virtual/$trafficIndex HTTP/1.1`r`n`r`n") }
                        else { $trafficWriter.Write("HTTP/1.1 200 OK`r`nContent-Type: text/plain`r`nContent-Length: 2`r`n`r`nok") }
                    } finally { $trafficWriter.Dispose() }
                }
                # Exercise aligned, overlapping intervals as well as one unknown start.
                if ($trafficIndex -lt 20000) {
                    $trafficStart = [datetime]::new(2026, 10, 8, 19, 0, 0, [DateTimeKind]::Utc).AddMilliseconds($trafficIndex * 10)
                    $trafficEnd = $trafficStart.AddMilliseconds(20 + ($trafficIndex % 10) * 30)
                    $trafficStartText = $trafficStart.ToString('o', [Globalization.CultureInfo]::InvariantCulture)
                    $trafficEndText = $trafficEnd.ToString('o', [Globalization.CultureInfo]::InvariantCulture)
                    $trafficTiming = $trafficZip.CreateEntry("raw/${trafficIndex}_m.xml")
                    $trafficTimingWriter = [IO.StreamWriter]::new($trafficTiming.Open(), [Text.UTF8Encoding]::new($false))
                    try { $trafficTimingWriter.Write("<Session><SessionTimers ClientBeginRequest=`"$trafficStartText`" ClientDoneResponse=`"$trafficEndText`"/></Session>") }
                    finally { $trafficTimingWriter.Dispose() }
                }
            }
        } finally { $trafficZip.Dispose() }
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
        $pageHtml = '<!doctype html><html><head><meta charset="utf-8"><title>Captured fixture</title><link rel="stylesheet" href="/page.css"></head><body><h1 id="captured-heading">Captured page fixture</h1><img id="captured-image" src="/pixel.svg"><p id="experiment-result">Script not run</p><script src="/experiment.js"></script><script>globalThis.capturedScriptRan=true;Promise.all([fetch("/variant",{method:"POST",headers:{"X-Preview":"dark"},body:"beta"}).then(async r=>({status:r.status,body:await r.text()})),fetch("/variant",{method:"POST",headers:{"X-Preview":"dark"},body:"alpha"}).then(async r=>({status:r.status,body:await r.text()}))]).then(rows=>globalThis.variantResults=rows);(async()=>{const rows=[];for(let i=0;i<3;i++){rows.push(await (await fetch("/sequence")).text());}globalThis.sequenceResults=rows;})();fetch("/missing").then(async r=>{globalThis.missingResult={status:r.status,body:await r.text()};},()=>{globalThis.missingResult={failed:true};});</script></body></html>'
        $pageMembers = [ordered]@{}
        $pageResources = @(
            @{ Id=1; Url='https://example.invalid/captured-page'; Type='text/html'; RequestHeaders="Sec-Fetch-Dest: document`r`nSec-Fetch-Mode: navigate`r`n"; ResponseHeaders="Content-Security-Policy: default-src 'self'; script-src 'self' 'unsafe-inline'`r`nCache-Control: max-age=60`r`n"; Body=$pageHtml },
            @{ Id=10; Url='https://example.invalid/experiment.js'; Type='text/javascript'; RequestHeaders="Sec-Fetch-Dest: script`r`nReferer: https://example.invalid/captured-page`r`n"; ResponseHeaders="Cache-Control: max-age=3600`r`n"; Body='globalThis.experimentScriptValue="original";fetch("/experiment-api").then(r=>r.json()).then(value=>{globalThis.experimentApi=value;document.getElementById("experiment-result").textContent=globalThis.experimentScriptValue+": "+value.value;});' },
            @{ Id=11; Url='https://example.invalid/experiment-api'; Type='application/json'; RequestHeaders="Sec-Fetch-Dest: empty`r`nReferer: https://example.invalid/captured-page`r`n"; ResponseHeaders="Cache-Control: private, max-age=60`r`n"; Body='{"token":"captured-token","value":42}' },
            @{ Id=9; Url='https://example.invalid/captured-post'; Method='POST'; RequestBody='form'; Type='text/html'; Body='<h1 id="post-heading">Captured POST fallback</h1>' },
            @{ Id=8; Url='https://example.invalid/captured-frame'; Type='text/html'; RequestHeaders="Sec-Fetch-Dest: iframe`r`nSec-Fetch-Mode: navigate`r`n"; Body='<h1>Captured frame fixture</h1>' },
            @{ Id=2; Url='https://example.invalid/page.css'; Type='text/css'; RequestHeaders="Sec-Fetch-Dest: style`r`n"; ResponseHeaders="Vary: User-Agent`r`n"; Body='h1 { color: rgb(0, 128, 0); }' },
            @{ Id=4; Url='https://example.invalid/variant'; Method='POST'; RequestBody='alpha'; RequestHeaders="X-Preview: light`r`n"; ResponseHeaders="Vary: X-Preview`r`n"; Type='text/plain'; Body='light variant' },
            @{ Id=5; Url='https://example.invalid/variant'; Method='POST'; RequestBody='beta'; RequestHeaders="X-Preview: dark`r`n"; ResponseHeaders="Vary: X-Preview`r`n"; Type='text/plain'; Body='dark variant' },
            @{ Id=6; Url='https://example.invalid/sequence'; Type='text/plain'; ResponseHeaders="Cache-Control: no-store`r`n"; Body='first response' },
            @{ Id=7; Url='https://example.invalid/sequence'; Type='text/plain'; ResponseHeaders="Cache-Control: no-store`r`n"; Body='next response' },
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
        if ($ViewerChecks -or $PortableCaseChecks) {
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
    if ($PortableCaseChecks) {
        Push-Location $desktopUi
        try {
            & node scripts/smoke-portable-cases.mjs --port "$DevToolsPort" --output $probeRoot --process-id "$($process.Id)" --close-helper (Join-Path $PSScriptRoot 'close-desktop-probe-window.ps1')
            if ($LASTEXITCODE -ne 0) { throw 'Native portable case checks failed.' }
        } finally { Pop-Location }
        return [pscustomobject]@{ PortableCasesVerified = $true; EvidenceRoot = $probeRoot }
    }
    if ($FormatConverterChecks) {
        Push-Location $desktopUi
        try {
            & node scripts/smoke-format-converter.mjs --port "$DevToolsPort" --output $probeRoot --process-id "$($process.Id)" --executable $executable --close-helper (Join-Path $PSScriptRoot 'close-desktop-probe-window.ps1')
            if ($LASTEXITCODE -ne 0) { throw 'Native format converter checks failed.' }
        } finally { Pop-Location }
        return [pscustomobject]@{ FormatConverterVerified = $true; EvidenceRoot = $probeRoot }
    }
    if ($StartupOnly) {
        $result = [pscustomobject]@{ StartupVerified = $true; HostedPolicy = [bool]$HostedRunnerDevToolsPolicy; Runtime = (Invoke-RestMethod -Uri "http://127.0.0.1:$DevToolsPort/json/version").Browser }
        $result | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $probeRoot 'startup.json') -Encoding utf8NoBOM
        return $result
    }

    function Save-DesktopMemorySample([string]$Phase) {
        $measuredProcess = Get-Process -Id $process.Id -ErrorAction Stop
        $sample = [ordered]@{
            Phase = $Phase
            TimestampUtc = [DateTimeOffset]::UtcNow.ToString('o')
            ProcessId = $measuredProcess.Id
            WorkingSetBytes = $measuredProcess.WorkingSet64
            PrivateBytes = $measuredProcess.PrivateMemorySize64
        }
        $sample | ConvertTo-Json -Compress | Add-Content -LiteralPath (Join-Path $probeRoot 'memory-samples.jsonl') -Encoding utf8NoBOM
        return [pscustomobject]$sample
    }
    $startingMemory = Save-DesktopMemorySample 'Functional checks start'
    $startingWorkingSet = $startingMemory.WorkingSetBytes
    $invalidTrace = Join-Path $probeRoot 'invalid.tmcap'
    [IO.File]::WriteAllText($invalidTrace, 'Invalid capture fixture')
    Push-Location $desktopUi
    try {
        # The timed soak runs below, after functional loading has settled.
        $smokeArguments = @('run', 'smoke:webview', '--', '--port', "$DevToolsPort", '--soak-minutes', '0', '--invalid-trace', $invalidTrace)
        if ($ScreenshotPath) {
            $smokeArguments += @('--screenshot', $ScreenshotPath)
        }
        if ($AutomationScreenshotPath) {
            $smokeArguments += @('--automation-screenshot', $AutomationScreenshotPath)
        }
        if ($KeyRecoveryChecks) { $smokeArguments += '--key-recovery' }
        if ($visualizerTrace) { $smokeArguments += @('--visualizer-trace', $visualizerTrace) }
        if ($ViewerChecks) {
            $viewerArguments = @('scripts/smoke-viewers.mjs', '--port', "$DevToolsPort", '--source', $viewerSource, '--executable', $executable)
            $viewerArguments += @('--page-source', $pageSource)
            if (-not $CapturedPageChecks) { $viewerArguments += @('--traffic-source', $trafficSource) }
            $viewerArguments += @('--profile-root', $probeRoot)
            $viewerArguments += @('--process-id', "$($process.Id)", '--close-helper', (Join-Path $PSScriptRoot 'close-desktop-probe-window.ps1'))
            if ($ScreenshotPath) { $viewerArguments += @('--screenshot', $ScreenshotPath) }
            if ($NativeTracePath) { $viewerArguments += @('--native-source', $NativeTracePath) }
            if ($TraceSaveChecks) { $viewerArguments += '--trace-save-only' }
            & node @viewerArguments | ForEach-Object {
                Write-Host $_
                Save-DesktopMemorySample ([string]$_) | Out-Null
            }
        } else { & npm @smokeArguments }
    } finally {
        Pop-Location
    }
    $endingMemory = Save-DesktopMemorySample 'Functional checks end'
    if ($extensionSource -and -not (Test-Path -LiteralPath (Join-Path $extensionSource 'main.js') -PathType Leaf)) { throw 'Unregistering the development extension deleted its source file.' }
    $endingWorkingSet = $endingMemory.WorkingSetBytes
    if ($KeyRecoveryChecks -and [Convert]::ToHexString([IO.File]::ReadAllBytes($fixturePath)) -ne [Convert]::ToHexString($fixtureBytes)) {
        throw 'Inspecting or canceling DPAPI recovery changed the existing private key.'
    }
    # Functional fixtures change the retained catalog (including 20,000 requests).
    # Bound repeated status work after loading, keeping total growth as diagnostics.
    $memoryReportPath = Join-Path $probeRoot 'status-memory.json'
    $memoryWindowRole = if ($ViewerChecks) { 'viewer' } else { 'main' }
    Push-Location $desktopUi
    try {
        & node scripts/smoke-desktop-memory.mjs --port "$DevToolsPort" --profile-root $probeRoot --process-id "$($process.Id)" --memory-report $memoryReportPath --soak-minutes "$SoakMinutes" --window-role $memoryWindowRole
        if ($LASTEXITCODE -ne 0) { throw "Native status-refresh memory gate failed. Report: $memoryReportPath" }
    } finally { Pop-Location }
    $memoryReport = Get-Content -LiteralPath $memoryReportPath -Raw | ConvertFrom-Json
    if ($memoryReport.schemaVersion -ne 1 -or $memoryReport.processId -ne $process.Id -or $memoryReport.limitBytes -ne 268435456 -or $memoryReport.status -ne 'passed' -or $memoryReport.workload.windowRole -ne $memoryWindowRole -or $memoryReport.workload.minutes -ne $SoakMinutes -or $memoryReport.workload.iterations -lt $(if ($SoakMinutes) { 1 } else { 100 })) {
        throw "Native status-refresh memory report is missing or invalid: $memoryReportPath"
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
        AccessibilityVerified = -not $ViewerChecks -or $TraceSaveChecks
        HighContrastVerified = -not $ViewerChecks
        HighDpiVerified = -not $ViewerChecks
        LocalizationLengthVerified = -not $ViewerChecks
        ViewerFlowVerified = [bool]($ViewerChecks -and -not $TraceSaveChecks)
        TraceSaveProgressVerified = [bool]$ViewerChecks
        KeyRecoveryVerified = [bool]$KeyRecoveryChecks
        SoakMinutes = $SoakMinutes
        WorkingSetGrowthBytes = $endingWorkingSet - $startingWorkingSet
        StatusWorkingSetGrowthBytes = $memoryReport.growthBytes
        MemoryGrowthVerified = $true
        MemoryReport = $memoryReportPath
        SingleInstanceVerified = $true
    }
} finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
        if (-not $process.WaitForExit(10000)) { throw 'The owned desktop probe did not finish stopping.' }
    }
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $priorArguments
    $env:LOCALAPPDATA = $priorLocalAppData
    $env:TEMP = $priorTemp
    $env:TMP = $priorTmp
    $env:TRANSMOG_DESKTOP_CASE_TEST = $priorCaseTest
    if ($policySet) {
        if ($null -ne $priorPolicyKind) { (Get-Item -LiteralPath $policyPath).SetValue($policyName, $priorPolicy, $priorPolicyKind) }
        else { Remove-ItemProperty -LiteralPath $policyPath -Name $policyName -ErrorAction Stop }
    }
    if ($createdPolicyKey) {
        $policyKey = Get-Item -LiteralPath $policyPath
        if ($policyKey.ValueCount -eq 0 -and $policyKey.SubKeyCount -eq 0) { Remove-Item -LiteralPath $policyPath }
    }
}
