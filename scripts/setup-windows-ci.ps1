[CmdletBinding()]
param([switch]$VerifyDownloadsOnly)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$toolsRoot = Join-Path $repositoryRoot '.tools'
$downloadsRoot = Join-Path $toolsRoot 'downloads'

# Official release distributions, matched to the supported Windows build profile.
# GitHub release asset digests were checked when these pins were introduced.
# NASM's digest was calculated from its official HTTPS release distribution.
$nativeTools = @(
    @{
        Name = 'LLVM'; Version = '22.1.4'; Kind = 'nsis'; Destination = 'llvm'
        File = 'LLVM-22.1.4-win64.exe'
        Url = 'https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.4/LLVM-22.1.4-win64.exe'
        Sha256 = '21B70C77E26E69CC14AA6E1023568868476748586BF14A978915ED6B3CF76EA1'
    },
    @{
        Name = 'CMake'; Version = '4.4.4'; Kind = 'zip'; Destination = 'cmake'
        File = 'cmake-4.4.4-windows-x86_64.zip'
        Url = 'https://github.com/Kitware/CMake/releases/download/v4.4.4/cmake-4.4.4-windows-x86_64.zip'
        Sha256 = 'BACE36E94B31C68AB6FA295F26DFA11219E0701CF7C94B0284A7D1CB13DAC536'
    },
    @{
        Name = 'Ninja'; Version = '1.13.2'; Kind = 'zip'; Destination = 'ninja'
        File = 'ninja-win.zip'
        Url = 'https://github.com/ninja-build/ninja/releases/download/v1.13.2/ninja-win.zip'
        Sha256 = '07FC8261B42B20E71D1720B39068C2E14FFCEE6396B76FB7A795FB460B78DC65'
    },
    @{
        Name = 'NASM'; Version = '3.02'; Kind = 'zip'; Destination = 'nasm'
        File = 'nasm-3.02-win64.zip'
        Url = 'https://www.nasm.us/pub/nasm/releasebuilds/3.02/win64/nasm-3.02-win64.zip'
        Sha256 = '161D0BFAFF53C2F9E9F3E69FD0672323EBABAFD1268976A5CEC11BE92A19AEE7'
    }
)

New-Item -ItemType Directory -Force -Path $downloadsRoot | Out-Null
foreach ($tool in $nativeTools) {
    $archive = Join-Path $downloadsRoot $tool.File
    if (-not (Test-Path -LiteralPath $archive)) {
        Write-Host "Downloading $($tool.Name) $($tool.Version)."
        Invoke-WebRequest -Uri $tool.Url -OutFile $archive
    }
    $actualHash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
    if ($actualHash -ne $tool.Sha256) {
        throw "$($tool.Name) archive SHA-256 mismatch: $actualHash"
    }
    Write-Host "Verified $($tool.Name) $($tool.Version): $actualHash"
    if ($VerifyDownloadsOnly) { continue }

    $destination = Join-Path $toolsRoot $tool.Destination
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    if ($tool.Kind -eq 'nsis') {
        $sevenZip = Get-Command 7z -ErrorAction SilentlyContinue
        if ($null -eq $sevenZip) {
            throw 'Extracting the official LLVM distribution requires 7-Zip (included in windows-2025).'
        }
        # Extract the portable files; do not run the system-wide installer.
        & $sevenZip.Source x -y "-o$destination" $archive | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "LLVM extraction failed: $LASTEXITCODE" }
    } else {
        Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force
    }
}

if ($VerifyDownloadsOnly) { return }

$windowsSdk = 'C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0'
if (-not (Test-Path -LiteralPath $windowsSdk)) {
    throw 'The hosted runner must provide the Windows 11 SDK 10.0.26100 and Visual Studio C++ tools.'
}
. (Join-Path $PSScriptRoot 'dev-env.ps1') -Check

$toolVersions = @(
    @{ Command = 'clang'; Arguments = @('--version'); Expected = '22.1.4' },
    @{ Command = 'cmake'; Arguments = @('--version'); Expected = '4.4.4' },
    @{ Command = 'ninja'; Arguments = @('--version'); Expected = '1.13.2' },
    @{ Command = 'nasm'; Arguments = @('-v'); Expected = '3.02' }
)
foreach ($toolVersion in $toolVersions) {
    $commandOutput = (& $toolVersion.Command @($toolVersion.Arguments) | Out-String).Trim()
    if ($commandOutput -notmatch ('\b' + [regex]::Escape($toolVersion.Expected) + '\b')) {
        throw "Unexpected $($toolVersion.Command) version: $commandOutput"
    }
    Write-Host ($commandOutput -split "`n" | Select-Object -First 1)
}
