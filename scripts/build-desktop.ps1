param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Debug',
    [switch]$LockedDependencies
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$uiDirectory = Join-Path $repositoryRoot 'apps\desktop\ui'

Push-Location $uiDirectory
try {
    npm run check
} finally {
    Pop-Location
}

$arguments = @('build', '-p', 'transmog-desktop')
if ($Configuration -eq 'Release') {
    $arguments += '--release'
}
if ($LockedDependencies) {
    $arguments += '--locked'
}
& cargo @arguments
if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed with exit code $LASTEXITCODE"
}
