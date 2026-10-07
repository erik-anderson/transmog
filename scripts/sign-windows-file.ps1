[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$FilePath,
    [Parameter(Mandatory)][string]$MetadataPath,
    [Parameter(Mandatory)][string]$ClientDll,
    [Parameter(Mandatory)][string]$ExpectedPublisher
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
. (Join-Path $PSScriptRoot 'windows-release-common.ps1')
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$resolvedFile = (Resolve-Path -LiteralPath $FilePath).Path
$allowedRoot = [System.IO.Path]::GetFullPath($repositoryRoot).TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
if (-not $resolvedFile.StartsWith($allowedRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Signing input is outside the release workspace.' }
if ([System.IO.Path]::GetExtension($resolvedFile).ToLowerInvariant() -notin @('.exe', '.dll', '.tmp')) { throw 'Signing input is not a Windows packaging binary.' }
$inputFile = Get-Item -LiteralPath $resolvedFile
if ($inputFile.LinkType) { throw 'Signing file links is not allowed.' }
$stream = [System.IO.File]::OpenRead($resolvedFile)
try { if ($stream.ReadByte() -ne 77 -or $stream.ReadByte() -ne 90) { throw 'Signing input has no PE header.' } } finally { $stream.Dispose() }
$metadata = Get-Content -Raw -LiteralPath $MetadataPath | ConvertFrom-Json
$endpoint = [uri]$metadata.Endpoint
if ($endpoint.Scheme -ne 'https' -or $endpoint.Host -notmatch '^[a-z0-9]+\.codesigning\.azure\.net$' -or $endpoint.UserInfo) { throw 'Unexpected signing endpoint.' }
$signTool = Resolve-WindowsSignTool
& $signTool sign /fd SHA256 /tr 'http://timestamp.acs.microsoft.com' /td SHA256 /dlib $ClientDll /dmdf $MetadataPath $resolvedFile | Out-Host
if ($LASTEXITCODE -ne 0) { throw "Artifact Signing failed: $LASTEXITCODE" }
$evidence = Get-WindowsSignatureEvidence -FilePath $resolvedFile -ExpectedPublisher $ExpectedPublisher
if ($env:TRANSMOG_SIGNING_JOURNAL) {
    $evidence | ConvertTo-Json -Compress | Add-Content -LiteralPath $env:TRANSMOG_SIGNING_JOURNAL -Encoding utf8NoBOM
}
$evidence
