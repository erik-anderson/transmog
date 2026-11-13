[CmdletBinding()]
param([switch]$VerifyDownloadsOnly)

$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$downloadsRoot = Join-Path $repositoryRoot '.tools/downloads'
$installation = Join-Path $repositoryRoot '.tools/msvc'
$version = '14.51.36231'
$bootstrapper = Join-Path $downloadsRoot 'vs_BuildTools-18.11.3.exe'
$expectedHash = 'EA54B67BFAC576B6DD097F84EEAF6893780D2A613F90448FF038E8A2777A0AFB'
# Fixed-version bootstrapper from Microsoft's Visual Studio release history.
# Its catalog, rather than the runner image's rolling Latest component, selects
# the headers, CRT libraries and native linker for both architectures.
$url = 'https://download.visualstudio.microsoft.com/download/pr/5f474104-723f-4007-bf4c-92e42a23dcc6/ea54b67bfac576b6dd097f84eeaf6893780d2a613f90448ff038e8a2777a0afb/vs_BuildTools.exe'
New-Item -ItemType Directory -Force -Path $downloadsRoot | Out-Null
if (-not (Test-Path -LiteralPath $bootstrapper)) {
    Invoke-WebRequest -Uri $url -OutFile $bootstrapper
}
if ((Get-FileHash -LiteralPath $bootstrapper -Algorithm SHA256).Hash -cne $expectedHash) {
    throw 'Fixed MSVC bootstrapper SHA-256 mismatch.'
}
$signature = Get-AuthenticodeSignature -LiteralPath $bootstrapper
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '(^|, )O=Microsoft Corporation(,|$)') {
    throw 'Fixed MSVC bootstrapper must have a valid Microsoft signature.'
}
if ($VerifyDownloadsOnly) { return }

$toolset = Join-Path $installation "VC/Tools/MSVC/$version"
if (-not (Test-Path -LiteralPath $toolset)) {
    # Both target libraries are needed on x64 producers; ARM64 hosts use native
    # tools from the same fixed catalog. Install privately, without updating the
    # image's Visual Studio installation or following its moving channel.
    $arguments = @('--quiet', '--wait', '--norestart', '--nocache',
        '--installPath', ('"' + $installation + '"'),
        '--add', 'Microsoft.VisualStudio.Component.VC.14.51.x86.x64',
        '--add', 'Microsoft.VisualStudio.Component.VC.14.51.ARM64')
    $process = Start-Process -FilePath $bootstrapper -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -notin @(0, 3010)) {
        throw "Fixed MSVC installation failed: $($process.ExitCode)"
    }
}
foreach ($relative in @('include/vcruntime.h', 'include/yvals_core.h',
        'lib/x64/libcmt.lib', 'lib/x64/msvcrt.lib', 'lib/x64/vcruntime.lib',
        'lib/arm64/libcmt.lib', 'lib/arm64/msvcrt.lib', 'lib/arm64/vcruntime.lib')) {
    if (-not (Test-Path -LiteralPath (Join-Path $toolset $relative) -PathType Leaf)) {
        throw "Fixed MSVC profile is incomplete: $relative"
    }
}
$hostTools = if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq [Runtime.InteropServices.Architecture]::Arm64) { 'Hostarm64/arm64' } else { 'Hostx64/x64' }
if (-not (Test-Path -LiteralPath (Join-Path $toolset "bin/$hostTools/link.exe") -PathType Leaf)) {
    throw 'Fixed MSVC native linker is missing.'
}
Write-Host "Verified fixed MSVC $version from Visual Studio Build Tools 18.11.3."
$files = @{}
foreach ($file in @(Get-ChildItem -LiteralPath (Join-Path $toolset 'include') -File | Where-Object Name -In @('vcruntime.h', 'yvals_core.h')) +
        @(Get-Item -LiteralPath (Join-Path $toolset "bin/$hostTools/link.exe"))) {
    $files[[IO.Path]::GetRelativePath($toolset, $file.FullName)] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
}
$report = Join-Path $repositoryRoot 'artifacts/windows-msvc.json'
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $report) | Out-Null
@{ Version = $version; Distribution = '18.11.3'; BootstrapperSha256 = $expectedHash.ToLowerInvariant();
    Installation = $installation; HostTools = $hostTools; Files = $files } |
    ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $report -Encoding utf8
