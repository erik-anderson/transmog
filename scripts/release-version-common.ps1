. (Join-Path $PSScriptRoot 'python-tools.ps1')

# Windows integration uses the same version decisions as the portable runners.
function Invoke-ReleaseVersionBridge($Request) {
    $result = ($Request | ConvertTo-Json -Depth 10 -Compress) | & (Get-TransmogPython) (Join-Path $PSScriptRoot 'release-version-bridge.py')
    if ($LASTEXITCODE) { throw 'Release version validation failed.' }
    ($result -join "`n") | ConvertFrom-Json
}
function ConvertTo-ReleaseSemVer([string]$Version) { Invoke-ReleaseVersionBridge @{ operation='semver'; version=$Version } }
function Get-ReleaseVersionState([string]$RepositoryRoot) { Invoke-ReleaseVersionBridge @{ operation='state'; root=$RepositoryRoot } }
function Get-NextReleaseVersion([string]$Version) { Invoke-ReleaseVersionBridge @{ operation='next'; version=$Version } }
function Get-ReleaseBranchName([string]$Version) { Invoke-ReleaseVersionBridge @{ operation='branch'; version=$Version } }
function Assert-ReleaseSourceBranch([string]$Branch, [string]$Version) { Invoke-ReleaseVersionBridge @{ operation='assert-branch'; branch=$Branch; version=$Version } }
function Resolve-ReleaseType($State, [string]$Branch, [string]$RequestedType = 'Branch default') { Invoke-ReleaseVersionBridge @{ operation='resolve'; state=$State; branch=$Branch; requested=$RequestedType } }
function Get-MainReleaseLineDecision($State, [string]$ReleaseLineVersion) { Invoke-ReleaseVersionBridge @{ operation='main-decision'; state=$State; version=$ReleaseLineVersion } }
function Get-PostReleaseDecision($State, [string]$PublishedVersion, [string]$Branch = 'main', [string]$PublishedType = 'Beta') { Invoke-ReleaseVersionBridge @{ operation='post-decision'; state=$State; version=$PublishedVersion; branch=$Branch; publishedType=$PublishedType } }

# Keep this native PowerShell API boundary for Windows publication integration.
function Assert-CanaryReleaseLineAvailable([string]$Version, [string]$Repository, $Headers) {
    $branch = Get-ReleaseBranchName $Version
    $reserved = $null
    try { $reserved = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repository/git/ref/heads/$branch" -Headers $Headers } catch {
        if ([int]$_.Exception.Response.StatusCode -ne 404) { throw }
    }
    if ($reserved) { throw "Release line $branch is reserved. Advance main to its next minor version before releasing a Canary." }
}
