param([Parameter(Mandatory)][string]$ReleaseRoot, [Parameter(Mandatory)][string]$ReportPath)
. (Join-Path $PSScriptRoot 'windows-release-common.ps1')
$manifest = Assert-SignedRelease -ReleaseRoot $ReleaseRoot -Commit $env:GITHUB_SHA -RunId $env:GITHUB_RUN_ID
if ($env:ACTIONS_ID_TOKEN_REQUEST_TOKEN) { throw 'Installer execution must not have Azure OIDC permission.' }
$installer = Join-Path $ReleaseRoot $manifest.Installer
$signatures = [System.Collections.Generic.List[object]]::new()
$signatures.Add((Get-WindowsSignatureEvidence -FilePath $installer -ExpectedPublisher $manifest.Publisher))
$cli = Join-Path $ReleaseRoot $manifest.Cli
$signatures.Add((Get-WindowsSignatureEvidence -FilePath $cli -ExpectedPublisher $manifest.Publisher))
if ((Get-Item -LiteralPath $cli).VersionInfo.FileVersion -cne $manifest.Version) { throw 'Standalone CLI resource version differs from the release.' }
$installDirectory = Join-Path $env:RUNNER_TEMP "transmog-install-$env:GITHUB_RUN_ID"
if (Test-Path -LiteralPath $installDirectory) { throw 'The installation test requires a fresh directory.' }
function Invoke-MaintenanceProcess([string]$FilePath, [string[]]$Arguments) {
    $process = Start-Process -FilePath $FilePath -ArgumentList $Arguments -PassThru -WindowStyle Hidden
    if (-not $process.WaitForExit(180000)) { Stop-Process -Id $process.Id -Force; throw 'Installer or maintenance process timed out.' }
    if ($process.ExitCode -ne 0) { throw "Maintenance process failed with exit code $($process.ExitCode)." }
}
function Get-HostState {
    $proxy = Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'
    [ordered]@{
        ProxyEnable = $proxy.ProxyEnable; ProxyServer = $proxy.ProxyServer; ProxyOverride = $proxy.ProxyOverride; AutoConfigURL = $proxy.AutoConfigURL
        CurrentUserRoots = @(Get-ChildItem Cert:\CurrentUser\Root | ForEach-Object Thumbprint | Sort-Object)
        SazDefault = (Get-Item -LiteralPath 'HKCU:\Software\Classes\.saz' -ErrorAction SilentlyContinue)?.GetValue('')
        SazUserChoice = Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.saz\UserChoice' -ErrorAction SilentlyContinue | Select-Object ProgId, Hash
    } | ConvertTo-Json -Depth 3 -Compress
}
$before = Get-HostState
Invoke-MaintenanceProcess -FilePath $cli -Arguments @('--version')
# Exercise the signed standalone artifact with owned profiles and loopback HTTP.
# This never installs a root or changes proxy settings, and runs without OIDC.
& python (Join-Path $PSScriptRoot 'test-cli-support.py') --executable $cli --artifacts (Join-Path $env:RUNNER_TEMP "transmog-cli-smoke-$env:GITHUB_RUN_ID")
if ($LASTEXITCODE -ne 0) { throw 'Signed CLI capture/lifecycle qualification failed.' }
Invoke-MaintenanceProcess -FilePath $installer -Arguments @('/S', '/SAZ=1', "/D=$installDirectory")
if (Test-Path -LiteralPath (Join-Path $installDirectory 'transmog-cli.exe')) { throw 'The standalone CLI must not be bundled in the app installer.' }
$sazCommand = (Get-Item -LiteralPath 'HKCU:\Software\Classes\Transmog.Saz\shell\open\command').GetValue('')
if ($sazCommand -cne ('"' + (Join-Path $installDirectory 'transmog.exe') + '" "%1"')) { throw 'SAZ registration did not quote the executable and file path correctly.' }
Invoke-MaintenanceProcess -FilePath (Join-Path $installDirectory 'transmog.exe') -Arguments @('--verify-update-artifact', ('"' + (Resolve-Path -LiteralPath $installer).Path + '"'), ('"' + (Resolve-Path -LiteralPath "$installer.sig").Path + '"'), (ConvertTo-ReleaseSemVer $manifest.Version))
foreach ($name in @('transmog.exe', 'transmog-script-host.exe', 'transmog-preview-worker.exe', 'uninstall.exe')) {
    $signatures.Add((Get-WindowsSignatureEvidence -FilePath (Join-Path $installDirectory $name) -ExpectedPublisher $manifest.Publisher))
}
Invoke-MaintenanceProcess -FilePath (Join-Path $installDirectory 'transmog.exe') -Arguments @('--prepare-update')
Invoke-MaintenanceProcess -FilePath (Join-Path $installDirectory 'uninstall.exe') -Arguments @('/S')
$deadline = [DateTime]::UtcNow.AddSeconds(60)
while ((Test-Path -LiteralPath (Join-Path $installDirectory 'transmog.exe')) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 250 }
if (Test-Path -LiteralPath (Join-Path $installDirectory 'transmog.exe')) { throw 'Uninstall left the installed executable behind.' }
if ((Get-HostState) -cne $before) { throw 'The no-proxy/no-certificate installer smoke test changed host proxy or current-user roots.' }
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $ReportPath) | Out-Null
[ordered]@{
    Commit = $env:GITHUB_SHA; RunId = $env:GITHUB_RUN_ID; InstallerSha256 = $signatures[0].Sha256
    CliRuntimeVerified = $true
    UpdaterSignatureVerified = $true
    InstallVerified = $true; MaintenanceVerified = $true; UninstallVerified = $true; HostStateUnchanged = $true
    OperatingSystem = (Get-CimInstance Win32_OperatingSystem).Caption
    Signatures = $signatures.ToArray(); CleanWindows11Checklist = 'deferred by maintainer'
} | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $ReportPath -Encoding utf8NoBOM
Write-Host 'Signed installer, installed binaries, uninstaller, maintenance and removal passed on the hosted Windows runner.'
