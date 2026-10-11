# One-command Windows desktop build and launch; build logic stays in Python.
#Requires -Version 7.0
[CmdletBinding()]
param([ValidateSet('Debug', 'Release')][string]$Configuration = 'Debug')

. (Join-Path $PSScriptRoot 'python-tools.ps1')
$repositoryRoot = Split-Path $PSScriptRoot -Parent
Push-Location $repositoryRoot
try {
    Write-Host 'Checking the development toolchain...'
    Invoke-TransmogPython 'dev-env.py' @('--check')
    . (Join-Path $PSScriptRoot 'dev-env.ps1')

    Write-Host 'Installing desktop UI dependencies from the lockfile...'
    Invoke-TransmogPython 'dev-env.py' @('--run', 'npm', 'ci', '--prefix', (Join-Path $repositoryRoot 'apps/desktop/ui'))

    Write-Host "Building the desktop UI and native helpers ($Configuration)..."
    Invoke-TransmogPython 'build-desktop.py' @('--configuration', $Configuration, '--locked-dependencies')

    $targetDirectory = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
    if (-not [IO.Path]::IsPathRooted($targetDirectory)) {
        $targetDirectory = Join-Path $repositoryRoot $targetDirectory
    }
    $profile = if ($Configuration -eq 'Release') { 'release' } else { 'debug' }
    $desktopExecutable = Join-Path $targetDirectory "$profile/transmog.exe"
    if (-not (Test-Path -LiteralPath $desktopExecutable -PathType Leaf)) {
        throw "The desktop build did not produce $desktopExecutable."
    }
    Write-Host "Launching $desktopExecutable"
    & $desktopExecutable
} finally {
    Pop-Location
}
