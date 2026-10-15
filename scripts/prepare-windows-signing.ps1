param([Parameter(Mandatory)][string]$PayloadRoot)
. (Join-Path $PSScriptRoot 'windows-release-common.ps1')
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$manifest = Assert-ReleasePayload -PayloadRoot $PayloadRoot -Commit $env:GITHUB_SHA -RunId $env:GITHUB_RUN_ID
$platform = Get-WindowsReleaseTarget $manifest.Target
foreach ($entry in $manifest.Files | Where-Object { $_.Path -match '^(target/release/|apps/desktop/ui/dist/)' }) {
    $relativePath = $entry.Path -replace '^target/release', $platform.BuildSubdirectory
    $destination = Join-Path $repositoryRoot $relativePath
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
    Copy-Item -LiteralPath (Join-Path $PayloadRoot $entry.Path) -Destination $destination -Force
}
. (Join-Path $PSScriptRoot 'release-version-common.ps1')
$state = Get-ReleaseVersionState $repositoryRoot
if ($state.Version -cne $manifest.Version -or $state.Channel -cne $manifest.Channel) { throw 'Build version/channel differs from the checked-out source.' }
Write-Host "Verified same-run build payload at $($manifest.Commit)."
