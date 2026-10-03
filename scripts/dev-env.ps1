[CmdletBinding()]
param([switch]$Check)

$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$portableBins = @(
    (Join-Path $repositoryRoot '.tools\llvm\bin'),
    (Join-Path $repositoryRoot '.tools\cmake\cmake-4.4.4-windows-x86_64\bin'),
    (Join-Path $repositoryRoot '.tools\ninja'),
    (Join-Path $repositoryRoot '.tools\nasm\nasm-3.02')
) | Where-Object { Test-Path -LiteralPath $_ }

$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
$modernVcRoot = $null
$windowsSdkRoot = $null
if (Test-Path -LiteralPath $vswhere) {
    $vsInstall = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($vsInstall) {
        $vcToolsRoot = Join-Path $vsInstall 'VC\Tools\MSVC'
        $modernVcRoot = Get-ChildItem -LiteralPath $vcToolsRoot -Directory -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending |
            Select-Object -First 1 -ExpandProperty FullName
        $windowsSdkRoot = 'C:\Program Files (x86)\Windows Kits\10'
    }
}

if ($modernVcRoot -and (Test-Path -LiteralPath $windowsSdkRoot)) {
    $sdkVersions = Get-ChildItem -LiteralPath (Join-Path $windowsSdkRoot 'Include') -Directory
    $sdkVersion = $sdkVersions |
        Where-Object Name -Like '10.0.26100.*' |
        Sort-Object Name -Descending |
        Select-Object -First 1 -ExpandProperty Name
    if (-not $sdkVersion) {
        $sdkVersion = $sdkVersions |
            Sort-Object Name -Descending |
            Select-Object -First 1 -ExpandProperty Name
    }
    $portableBins += @(
        (Join-Path $modernVcRoot 'bin\Hostx64\x64'),
        (Join-Path $windowsSdkRoot "bin\$sdkVersion\x64")
    )
    $env:LIB = @(
        (Join-Path $modernVcRoot 'lib\x64'),
        (Join-Path $windowsSdkRoot "Lib\$sdkVersion\ucrt\x64"),
        (Join-Path $windowsSdkRoot "Lib\$sdkVersion\um\x64")
    ) -join ';'
    $env:INCLUDE = @(
        (Join-Path $modernVcRoot 'include'),
        (Join-Path $windowsSdkRoot "Include\$sdkVersion\ucrt"),
        (Join-Path $windowsSdkRoot "Include\$sdkVersion\shared"),
        (Join-Path $windowsSdkRoot "Include\$sdkVersion\um"),
        (Join-Path $windowsSdkRoot "Include\$sdkVersion\winrt")
    ) -join ';'
} else {
    $scopeSdk = 'C:\Program Files\Microsoft Visual Studio\18\Community\SDK\ScopeCppSDK\vc15'
}
if (-not $modernVcRoot -and (Test-Path -LiteralPath $scopeSdk)) {
    $portableBins += @(
        (Join-Path $scopeSdk 'VC\bin'),
        (Join-Path $scopeSdk 'SDK\bin')
    )
    $env:LIB = "$(Join-Path $scopeSdk 'VC\lib');$(Join-Path $scopeSdk 'SDK\lib')"
    $env:INCLUDE = @(
        (Join-Path $scopeSdk 'VC\include'),
        (Join-Path $scopeSdk 'SDK\include\ucrt'),
        (Join-Path $scopeSdk 'SDK\include\shared'),
        (Join-Path $scopeSdk 'SDK\include\um')
    ) -join ';'
}
$env:PATH = ($portableBins -join ';') + ";$env:PATH"

$required = @('rustc', 'cargo', 'clang', 'clang-cl', 'llvm-lib', 'link', 'cmake', 'ninja', 'nasm')
$missing = @()
foreach ($command in $required) {
    $resolved = Get-Command $command -ErrorAction SilentlyContinue
    if ($null -eq $resolved) {
        $missing += $command
    } elseif ($Check) {
        Write-Host ("{0,-10} {1}" -f $command, $resolved.Source)
    }
}
if ($missing.Count -ne 0) {
    throw "Missing native build commands: $($missing -join ', ')"
}

$env:CC = 'clang-cl'
$env:CXX = 'clang-cl'
$env:AR = 'llvm-lib'
$repositoryLibclang = Join-Path $repositoryRoot '.tools\llvm\bin'
if (Test-Path -LiteralPath (Join-Path $repositoryLibclang 'libclang.dll')) {
    $env:LIBCLANG_PATH = $repositoryLibclang
} elseif (-not $env:LIBCLANG_PATH) {
    $systemClangDirectory = Split-Path -Parent (Get-Command clang).Source
    if (Test-Path -LiteralPath (Join-Path $systemClangDirectory 'libclang.dll')) {
        $env:LIBCLANG_PATH = $systemClangDirectory
    }
}
$env:CMAKE_GENERATOR = 'Ninja'
$env:CMAKE_MAKE_PROGRAM = (Get-Command ninja).Source
Write-Host 'Configured LLVM/Clang, Ninja, and the Windows SDK linker for this PowerShell process.'
