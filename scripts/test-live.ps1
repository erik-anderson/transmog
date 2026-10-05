$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
. "$PSScriptRoot\dev-env.ps1"

cargo build --release --locked -p transmog
$project = Join-Path $PSScriptRoot '..\e2e\playwright'
Push-Location $project
try {
    npm ci
    npx playwright install chromium
    $env:TRANSMOG_BIN = (Resolve-Path (Join-Path $PSScriptRoot '..\target\release\transmog.exe')).Path
    $env:TRANSMOG_LIVE = '1'
    npm test
} finally {
    Pop-Location
}
