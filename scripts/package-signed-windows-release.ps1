param([Parameter(Mandatory)][string]$PayloadRoot, [Parameter(Mandatory)][string]$ClientDll)
. (Join-Path $PSScriptRoot 'windows-release-common.ps1')
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$build = Assert-ReleasePayload -PayloadRoot $PayloadRoot -Commit $env:GITHUB_SHA -RunId $env:GITHUB_RUN_ID
$releaseRoot = Join-Path $repositoryRoot 'artifacts\windows-signed'
New-Item -ItemType Directory -Force -Path $releaseRoot | Out-Null
$metadata = [ordered]@{
    Endpoint = $env:SIGNING_ENDPOINT
    CodeSigningAccountName = $env:SIGNING_ACCOUNT_NAME
    CertificateProfileName = $env:SIGNING_CERTIFICATE_PROFILE
    CorrelationId = "github-$env:GITHUB_RUN_ID-$env:GITHUB_RUN_ATTEMPT"
    ExcludeCredentials = @('EnvironmentCredential', 'ManagedIdentityCredential', 'WorkloadIdentityCredential', 'SharedTokenCacheCredential', 'VisualStudioCredential', 'VisualStudioCodeCredential', 'AzurePowerShellCredential', 'AzureDeveloperCliCredential', 'InteractiveBrowserCredential')
}
foreach ($required in @('Endpoint', 'CodeSigningAccountName', 'CertificateProfileName')) {
    if (-not $metadata[$required]) { throw "Missing signing configuration: $required" }
}
if (-not $env:SIGNING_PUBLISHER -or -not $env:SIGNING_DENIED_PROFILE -or $env:SIGNING_DENIED_PROFILE -eq $env:SIGNING_CERTIFICATE_PROFILE) { throw 'Publisher and a different denied probe profile are required.' }
$metadataPath = Join-Path $repositoryRoot 'artifacts\signing-metadata.json'
$metadata | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $metadataPath -Encoding utf8NoBOM
$env:TRANSMOG_SIGNING_JOURNAL = Join-Path $repositoryRoot 'artifacts\signatures.jsonl'
try {
    $package = & (Join-Path $PSScriptRoot 'package-windows.ps1') -BundleOnly -SigningMetadataPath $metadataPath -SigningClientDll $ClientDll -ExpectedPublisher $env:SIGNING_PUBLISHER |
        ForEach-Object { if ($null -ne $_.PSObject.Properties['Installer']) { $_ } else { $_ | Out-Host } }
    if (@($package).Count -ne 1) { throw 'Signed packaging did not return one installer.' }
    $installerEvidence = Get-WindowsSignatureEvidence -FilePath $package.Installer -ExpectedPublisher $env:SIGNING_PUBLISHER
    $cliEvidence = Get-WindowsSignatureEvidence -FilePath $package.Cli -ExpectedPublisher $env:SIGNING_PUBLISHER
    $journal = @(Get-Content -LiteralPath $env:TRANSMOG_SIGNING_JOURNAL | ForEach-Object { $_ | ConvertFrom-Json })
    foreach ($name in @('transmog.exe', 'transmog-script-host.exe', 'transmog-preview-worker.exe', 'transmog-cli.exe', 'NSISdl.dll', 'StartMenu.dll', 'System.dll', 'nsDialogs.dll', 'nsis_tauri_utils.dll')) {
        if (-not ($journal | Where-Object { $_.Name -ceq $name })) { throw "Missing signature evidence for $name" }
    }
    Copy-Item -LiteralPath $package.Installer -Destination (Join-Path $releaseRoot $installerEvidence.Name)
    Copy-Item -LiteralPath $package.Cli -Destination (Join-Path $releaseRoot $cliEvidence.Name)
    Copy-Item -Path (Join-Path $PayloadRoot 'evidence\*') -Destination $releaseRoot
    [ordered]@{ Publisher = $env:SIGNING_PUBLISHER; Files = $journal; Installer = $installerEvidence } | ConvertTo-Json -Depth 6 |
        Set-Content -LiteralPath (Join-Path $releaseRoot 'signature-report.json') -Encoding utf8NoBOM

    # A real data-plane request must be denied for the other, active profile.
    $deniedMetadata = Join-Path $repositoryRoot 'artifacts\denied-signing-metadata.json'
    $metadata.CertificateProfileName = $env:SIGNING_DENIED_PROFILE
    $metadata | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $deniedMetadata -Encoding utf8NoBOM
    $probeFile = Join-Path $repositoryRoot 'artifacts\permission-probe.exe'
    Copy-Item -LiteralPath (Join-Path $PayloadRoot 'target\release\transmog-preview-worker.exe') -Destination $probeFile
    $priorNativePreference = $PSNativeCommandUseErrorActionPreference
    try {
        $PSNativeCommandUseErrorActionPreference = $false
        $denial = (& (Resolve-WindowsSignTool) sign /fd SHA256 /dlib $ClientDll /dmdf $deniedMetadata $probeFile 2>&1 | Out-String)
        $denialExit = $LASTEXITCODE
    } finally { $PSNativeCommandUseErrorActionPreference = $priorNativePreference }
    Assert-OtherProfileSigningDenied -ExitCode $denialExit -Output $denial
    Write-Host 'Other-profile signing returned the required HTTP 403 denial.'
    [ordered]@{ OtherProfileSigningDenied = $true; HttpStatus = 403 } | ConvertTo-Json |
        Set-Content -LiteralPath (Join-Path $releaseRoot 'profile-permission-test.json') -Encoding utf8NoBOM
    $files = @(Get-ChildItem -LiteralPath $releaseRoot -File | ForEach-Object {
        [pscustomobject]@{ Name = $_.Name; Sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }
    })
    [ordered]@{ Commit = $env:GITHUB_SHA; RunId = $env:GITHUB_RUN_ID; SourceBranch = $build.SourceBranch; Version = $build.Version; Channel = $build.Channel; ReleaseType = $build.ReleaseType; Publisher = $env:SIGNING_PUBLISHER; Installer = $installerEvidence.Name; Cli = $cliEvidence.Name; CleanWindows11Checklist = 'deferred by maintainer'; Files = $files } |
        ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $releaseRoot 'release-manifest.json') -Encoding utf8NoBOM
    $files | ForEach-Object { "$($_.Sha256.ToLowerInvariant())  $($_.Name)" } | Set-Content -LiteralPath (Join-Path $releaseRoot 'SHA256SUMS') -Encoding utf8NoBOM
    if ($env:GITHUB_STEP_SUMMARY) { "Signed **$($installerEvidence.Name)** as **$env:SIGNING_PUBLISHER** with RFC 3161 timestamps. Other-profile signing returned HTTP 403. SHA-256: $($installerEvidence.Sha256)." | Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY }
} finally {
    $env:TRANSMOG_SIGNING_JOURNAL = $null
    Remove-Item -LiteralPath $metadataPath -Force -ErrorAction SilentlyContinue
}
