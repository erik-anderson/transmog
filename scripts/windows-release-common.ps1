# Shared checks used by the build, signing, qualification, and publication jobs.
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
. (Join-Path $PSScriptRoot 'release-version-common.ps1')

function Resolve-WindowsSignTool {
    $sdkBin = 'C:\Program Files (x86)\Windows Kits\10\bin'
    $signTools = @(Get-ChildItem -LiteralPath $sdkBin -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending | ForEach-Object {
            $candidate = Join-Path $_.FullName 'x64\signtool.exe'
            if (Test-Path -LiteralPath $candidate) { $candidate }
        })
    if ($signTools.Count -eq 0) { throw 'Windows SDK x64 SignTool is required.' }
    return $signTools[0]
}

function Assert-OtherProfileSigningDenied {
    param([int]$ExitCode, [string]$Output)
    if ($ExitCode -eq 0 -or $Output -notmatch '(?i)403|Forbidden') {
        throw "The other-profile test did not prove HTTP 403 denial: $Output"
    }
    # GitHub's PowerShell wrapper exits with LASTEXITCODE. The expected denial
    # passed this test, so its native failure must not fail the surrounding job.
    $global:LASTEXITCODE = 0
}

function Get-WindowsSignatureEvidence {
    param([Parameter(Mandatory)][string]$FilePath, [Parameter(Mandatory)][string]$ExpectedPublisher)
    $signature = Get-AuthenticodeSignature -LiteralPath $FilePath
    if ($signature.Status -ne 'Valid') { throw "Invalid Authenticode signature on $FilePath : $($signature.Status)" }
    $publisher = $signature.SignerCertificate.GetNameInfo([System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
    if ($publisher -cne $ExpectedPublisher) { throw "Unexpected publisher on $FilePath : $publisher" }
    if ($null -eq $signature.TimeStamperCertificate) { throw "Missing timestamp on $FilePath" }
    & (Resolve-WindowsSignTool) verify /pa /all /tw $FilePath | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "SignTool verification failed for $FilePath : $LASTEXITCODE" }
    [pscustomobject]@{
        Name = [System.IO.Path]::GetFileName($FilePath)
        Bytes = (Get-Item -LiteralPath $FilePath).Length
        Sha256 = (Get-FileHash -LiteralPath $FilePath -Algorithm SHA256).Hash
        Signature = [string]$signature.Status
        Publisher = $publisher
        CertificateThumbprint = $signature.SignerCertificate.Thumbprint
        CertificateIssuer = $signature.SignerCertificate.Issuer
        TimestampCertificate = $signature.TimeStamperCertificate.Thumbprint
    }
}

function Assert-ReleasePayload {
    param([Parameter(Mandatory)][string]$PayloadRoot, [Parameter(Mandatory)][string]$Commit, [Parameter(Mandatory)][string]$RunId)
    $PayloadRoot = (Get-Item -LiteralPath $PayloadRoot).FullName
    $manifest = Get-Content -Raw -LiteralPath (Join-Path $PayloadRoot 'build-manifest.json') | ConvertFrom-Json
    if ($manifest.Commit -cne $Commit -or $manifest.RunId -cne $RunId -or $manifest.Target -ne 'x86_64-pc-windows-msvc') {
        throw 'Build payload provenance does not match this release run.'
    }
    $root = [System.IO.Path]::GetFullPath($PayloadRoot).TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
    $seen = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $manifest.Files) {
        if ([System.IO.Path]::IsPathRooted($entry.Path)) { throw 'An artifact path is absolute.' }
        if (-not $seen.Add($entry.Path) -or $entry.Path -match '[:\\]' -or $entry.Sha256 -notmatch '^[0-9a-fA-F]{64}$') { throw 'Invalid or duplicate artifact entry.' }
        if ($entry.Path -notmatch '^(target/release/[^/]+\.exe|apps/desktop/ui/dist/.+|evidence/[^/]+)$') { throw 'Unexpected build payload path.' }
        $path = [System.IO.Path]::GetFullPath((Join-Path $PayloadRoot $entry.Path))
        if (-not $path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) { throw 'An artifact path escapes the payload root.' }
        $file = Get-Item -LiteralPath $path
        if ($file.LinkType) { throw 'Artifact links are not accepted.' }
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $entry.Sha256) { throw "Artifact checksum mismatch: $($entry.Path)" }
    }
    $actualFiles = @(Get-ChildItem -LiteralPath $PayloadRoot -File -Recurse | Where-Object { $_.FullName -ne (Join-Path $PayloadRoot 'build-manifest.json') })
    if ($actualFiles.Count -ne $seen.Count) { throw 'The build payload contains unlisted files.' }
    ConvertTo-ReleaseSemVer $manifest.Version | Out-Null
    Resolve-ReleaseType $manifest $manifest.SourceBranch $manifest.ReleaseType | Out-Null
    foreach ($binary in @('transmog.exe', 'transmog-script-host.exe', 'transmog-preview-worker.exe')) {
        $expectedPath = 'target/release/' + $binary
        if (@($manifest.Files | Where-Object { $_.Path -ceq $expectedPath }).Count -ne 1) { throw "Missing or duplicate payload binary: $binary" }
    }
    return $manifest
}

function Assert-SignedRelease {
    param([string]$ReleaseRoot, [string]$Commit, [string]$RunId)
    $manifest = Get-Content -Raw -LiteralPath (Join-Path $ReleaseRoot 'release-manifest.json') | ConvertFrom-Json
    if ($manifest.Commit -cne $Commit -or $manifest.RunId -cne $RunId) { throw 'Signed release provenance mismatch.' }
    ConvertTo-ReleaseSemVer $manifest.Version | Out-Null
    Resolve-ReleaseType $manifest $manifest.SourceBranch $manifest.ReleaseType | Out-Null
    foreach ($entry in $manifest.Files) {
        if ($entry.Name -notmatch '^[A-Za-z0-9_.-]+$' -or $entry.Sha256 -notmatch '^[A-Fa-f0-9]{64}$') { throw 'Invalid release asset.' }
        if ((Get-FileHash -LiteralPath (Join-Path $ReleaseRoot $entry.Name) -Algorithm SHA256).Hash -ne $entry.Sha256) { throw "Release asset checksum mismatch: $($entry.Name)" }
    }
    if (@($manifest.Files | Where-Object { $_.Name -ceq $manifest.Installer }).Count -ne 1 -or $manifest.Installer -notmatch '^Transmog_.+-setup\.exe$') { throw 'Missing or ambiguous release installer.' }
    return $manifest
}
