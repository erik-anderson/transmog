$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$denyRoot = Join-Path $repositoryRoot '.tools\cargo-deny'
New-Item -ItemType Directory -Force -Path $denyRoot | Out-Null
$denyArchive = Join-Path $denyRoot 'cargo-deny.tar.gz'
Invoke-WebRequest -Uri 'https://github.com/EmbarkStudios/cargo-deny/releases/download/0.20.2/cargo-deny-0.20.2-x86_64-pc-windows-msvc.tar.gz' -OutFile $denyArchive
if ((Get-FileHash -LiteralPath $denyArchive -Algorithm SHA256).Hash -ne '975A22143262FD27476D19EE00C7AF67978426E40E1DEE94EED6BBADE1CF87DC') { throw 'cargo-deny checksum mismatch.' }
& tar -xzf $denyArchive -C $denyRoot
if ($LASTEXITCODE -ne 0) { throw 'cargo-deny extraction failed.' }
$denyBinary = @(Get-ChildItem -LiteralPath $denyRoot -Filter 'cargo-deny.exe' -File -Recurse)
if ($denyBinary.Count -ne 1) { throw 'Expected one cargo-deny binary.' }
$env:PATH = $denyBinary[0].DirectoryName + ';' + $env:PATH
if ($env:GITHUB_PATH) { $denyBinary[0].DirectoryName | Add-Content -LiteralPath $env:GITHUB_PATH -Encoding utf8NoBOM }
Write-Host 'Verified cargo-deny 0.20.2 for the release policy gate.'
