param(
    [string]$RepositoryRoot = (Split-Path -Parent $PSScriptRoot),
    [string]$SourceBranch = $env:GITHUB_REF_NAME,
    [string]$RequestedType = $env:REQUESTED_RELEASE_TYPE
)
. (Join-Path $PSScriptRoot 'release-version-common.ps1')
$state = Get-ReleaseVersionState $RepositoryRoot
$releaseType = Resolve-ReleaseType $state $SourceBranch $RequestedType
$headers = @{ Authorization = "Bearer $env:GITHUB_TOKEN"; Accept = 'application/vnd.github+json'; 'X-GitHub-Api-Version' = '2022-11-28'; 'User-Agent' = 'Transmog-release-version-check' }
if ($SourceBranch -ceq 'main') {
    Assert-CanaryMajorAvailable $state.Version $env:GITHUB_REPOSITORY $headers
}
$release = $null
try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$env:GITHUB_REPOSITORY/releases/tags/v$($state.Version)" -Headers $headers
} catch {
    if ([int]$_.Exception.Response.StatusCode -ne 404) { throw }
}
if ($release -and -not $release.draft) { throw "Release v$($state.Version) is already published. Choose a new version before building." }
if ($env:GITHUB_ENV) { "RELEASE_TYPE=$releaseType" | Add-Content -LiteralPath $env:GITHUB_ENV }
Write-Host "Release version $($state.Version) is available for $releaseType."
