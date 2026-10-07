param()
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') { throw 'Runtime installation is limited to disposable GitHub-hosted runners.' }
function Get-EvergreenVersion {
    foreach ($path in @('HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}')) {
        $version = (Get-ItemProperty -LiteralPath $path -Name pv -ErrorAction SilentlyContinue).pv
        if ($version -match '^\d+\.\d+\.\d+\.\d+$' -and $version -ne '0.0.0.0') { return $version }
    }
}
$version = Get-EvergreenVersion
if (-not $version) {
    $installer = Join-Path $env:RUNNER_TEMP 'MicrosoftEdgeWebview2Setup.exe'
    Invoke-WebRequest -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $installer
    $signature = Get-AuthenticodeSignature -LiteralPath $installer
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.GetNameInfo([System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false) -cne 'Microsoft Corporation') { throw 'WebView2 bootstrapper does not have a valid Microsoft signature.' }
    $process = Start-Process -FilePath $installer -ArgumentList '/silent','/install' -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit(180000) -or $process.ExitCode -ne 0) { throw 'Evergreen runtime installation failed or timed out.' }
    $deadline = [DateTime]::UtcNow.AddMinutes(2)
    do { $version = Get-EvergreenVersion; if ($version) { break }; Start-Sleep -Seconds 1 } while ([DateTime]::UtcNow -lt $deadline)
}
if (-not $version) { throw 'Evergreen WebView2 Runtime is unavailable.' }
Write-Host "Hosted Evergreen WebView2 Runtime: $version"
