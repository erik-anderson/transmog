[CmdletBinding()]
param(
    [ValidateSet('both', 'cli', 'desktop')][string]$Target = 'both',
    [ValidateSet('disk', 'memory')][string]$Storage = 'disk',
    [string]$Python = 'python',
    [switch]$Smoke,
    [switch]$Headed,
    [switch]$SkipBuild,
    [switch]$SkipDependencyInstall,
    [switch]$SkipBrowserInstall,
    [ValidateRange(1, 10000)][int]$Navigations = 100,
    [ValidateRange(1, 10000)][int]$RequestsPerNavigation = 100,
    [ValidateRange(1, 32)][int]$Concurrency = 8,
    [ValidateRange(1, 240)][int]$TimeoutMinutes = 30,
    [string]$Output = 'artifacts/load'
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repositoryRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repositoryRoot
try {
    & $Python --version
    if (-not $SkipBuild) {
        . ./scripts/dev-env.ps1
        $packages = @('-p', 'transmog')
        if ($Target -ne 'cli') { $packages += @('-p', 'transmog-desktop') }
        cargo build --locked --release @packages
    }
    if (-not $SkipDependencyInstall) { npm --prefix e2e/playwright ci }
    if (-not $SkipBrowserInstall) {
        & node e2e/playwright/node_modules/playwright/cli.js install chromium
    }
    & node --test e2e/load/harness.test.mjs
    $cargoRoot = if ($env:CARGO_TARGET_DIR) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR, $repositoryRoot) } else { Join-Path $repositoryRoot 'target' }
    $cliName = if ($IsWindows) { 'transmog-cli.exe' } else { 'transmog-cli' }
    $arguments = @('e2e/load/run.mjs', '--target', $Target, '--storage', $Storage,
        '--python', $Python, '--concurrency', "$Concurrency", '--timeout-minutes', "$TimeoutMinutes",
        '--output', $Output, '--cli', (Join-Path $cargoRoot "release/$cliName"),
        '--desktop', (Join-Path $cargoRoot 'release/transmog.exe'))
    if ($Smoke) { $arguments += '--smoke' }
    if ($Headed) { $arguments += '--headed' }
    if (-not $Smoke -or $PSBoundParameters.ContainsKey('Navigations')) { $arguments += @('--navigations', "$Navigations") }
    if (-not $Smoke -or $PSBoundParameters.ContainsKey('RequestsPerNavigation')) { $arguments += @('--requests-per-navigation', "$RequestsPerNavigation") }
    & node @arguments
} finally { Pop-Location }
