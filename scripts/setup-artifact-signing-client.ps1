[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repositoryRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'windows-release-common.ps1')
$version = '1.0.128'
$expectedHash = '74BD7D27E6CE1051409C38D9B46BC8DF0400ECD643D51FFBF2AC00869061E40B'
$clientRoot = Join-Path $repositoryRoot '.tools\artifact-signing-client\1.0.128'
New-Item -ItemType Directory -Force -Path $clientRoot | Out-Null
$archive = Join-Path $clientRoot 'client.zip'
# Privileged signing jobs deliberately download their client afresh, without caches.
Invoke-WebRequest -Uri "https://api.nuget.org/v3-flatcontainer/microsoft.artifactsigning.client/$version/microsoft.artifactsigning.client.$version.nupkg" -OutFile $archive
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $expectedHash) { throw 'Artifact Signing client checksum mismatch.' }
Expand-Archive -LiteralPath $archive -DestinationPath $clientRoot -Force
$dlib = Join-Path $clientRoot 'bin\x64\Azure.CodeSigning.Dlib.dll'
if (-not (Test-Path -LiteralPath $dlib)) { throw 'The signing client x64 DLL is missing.' }
$runtimes = (& dotnet --list-runtimes | Out-String)
if ($runtimes -notmatch 'Microsoft.NETCore.App 8\.') { throw '.NET 8 x64 runtime is required for the signing client.' }
$signTool = Resolve-WindowsSignTool
if ($env:GITHUB_OUTPUT) {
    "client-dll=$dlib" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8NoBOM
    "sign-tool=$signTool" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8NoBOM
}
[pscustomobject]@{ Version = $version; Sha256 = $expectedHash; ClientDll = $dlib; SignTool = $signTool }
