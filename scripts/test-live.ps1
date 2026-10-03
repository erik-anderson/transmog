$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
. "$PSScriptRoot\dev-env.ps1"

cargo build --release --locked -p rustymiddle
$project = Join-Path $PSScriptRoot '..\e2e\playwright'
Push-Location $project
try {
    npm ci
    npx playwright install chromium
    $env:RUSTYMIDDLE_BIN = (Resolve-Path (Join-Path $PSScriptRoot '..\target\release\rustymiddle.exe')).Path
    $env:RUSTYMIDDLE_LIVE = '1'
    npm test
} finally {
    Pop-Location
}
