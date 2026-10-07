param([Parameter(Mandatory)][string]$PayloadRoot)
. (Join-Path $PSScriptRoot 'windows-release-common.ps1')
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$manifest = Assert-ReleasePayload -PayloadRoot $PayloadRoot -Commit $env:GITHUB_SHA -RunId $env:GITHUB_RUN_ID
foreach ($entry in $manifest.Files | Where-Object { $_.Path -match '^(target/release/|apps/desktop/ui/dist/)' }) {
    $destination = Join-Path $repositoryRoot $entry.Path
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
    Copy-Item -LiteralPath (Join-Path $PayloadRoot $entry.Path) -Destination $destination -Force
}
$configuredVersion = (Get-Content -Raw -LiteralPath (Join-Path $repositoryRoot 'apps\desktop\tauri.conf.json') | ConvertFrom-Json).version
if ($configuredVersion -cne $manifest.Version) { throw 'Build version differs from the checked-out Tauri configuration.' }
Write-Host "Verified same-run build payload at $($manifest.Commit)."
