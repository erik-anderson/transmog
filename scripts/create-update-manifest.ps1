param(
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$Notes = 'Download the verified release and update now, or when you quit Transmog.'
)
. (Join-Path $PSScriptRoot 'release-version-common.ps1')
$ErrorActionPreference = 'Stop'
$semVer = ConvertTo-ReleaseSemVer $Version
$installerName = [IO.Path]::GetFileName($Installer)
if ($installerName -cne "Transmog_${Version}_x64-setup.exe") { throw 'Unexpected updater installer name.' }
if (-not $env:TAURI_SIGNING_PRIVATE_KEY -and -not $env:TAURI_SIGNING_PRIVATE_KEY_PATH) { throw 'The protected release-signing environment needs TRANSMOG_UPDATER_PRIVATE_KEY before building a signed update.' }
$cli = Join-Path (Split-Path -Parent $PSScriptRoot) 'apps/desktop/ui/node_modules/@tauri-apps/cli/tauri.js'
# Sign only final Authenticode-signed bytes, with the semantic version bound into the signature.
$signOutput = & node $cli signer sign --app-version $semVer $Installer 2>&1
if ($LASTEXITCODE -ne 0) { throw 'Updater signing failed. Check the protected updater signing key and password.' }
$signature = (Get-Content -Raw -LiteralPath "$Installer.sig").Trim()
if ($signature.Length -gt 8192) { throw 'Unexpected updater signature size.' }
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
Copy-Item -LiteralPath "$Installer.sig" -Destination (Join-Path $OutputDirectory "$installerName.sig") -Force
[ordered]@{
    version = $semVer
    notes = $Notes
    platforms = @{ 'windows-x86_64-nsis' = @{
        url = "https://github.com/erik-anderson/transmog/releases/download/v$Version/$installerName"
        signature = $signature
    } }
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'latest.json') -Encoding utf8NoBOM
Write-Host "Prepared the signed update manifest for Transmog $Version."
