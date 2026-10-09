$ErrorActionPreference = 'Stop'

function ConvertTo-ReleaseSemVer([string]$Version) {
    if ($Version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' -or
        @($Version.Split('.') | Where-Object { [decimal]$_ -gt 65535 }).Count) {
        throw 'Use major.minor.patch without a v prefix or build metadata; Windows installer components must fit in 0-65535.'
    }
    $Version
}

function Get-ReleaseVersionState([string]$RepositoryRoot) {
    $state = Get-Content -Raw -LiteralPath (Join-Path $RepositoryRoot 'release-version.json') | ConvertFrom-Json
    $semVer = ConvertTo-ReleaseSemVer $state.version
    if ($state.channel -cnotin @('Canary', 'Release')) { throw 'Source channel must be Canary or Release.' }
    $releaseType = if ($state.releaseType) { [string]$state.releaseType } elseif ($state.channel -ceq 'Canary') { 'Canary' } else { 'Beta' }
    if ($releaseType -cnotin @('Canary', 'Beta', 'Stable') -or (($releaseType -ceq 'Canary') -ne ($state.channel -ceq 'Canary'))) { throw 'Release track does not match the source channel.' }
    [pscustomobject]@{ Version = [string]$state.version; Channel = [string]$state.channel; ReleaseType = $releaseType; SemVer = $semVer }
}

function Get-NextReleaseVersion([string]$Version) {
    ConvertTo-ReleaseSemVer $Version | Out-Null
    $parts = @($Version.Split('.') | ForEach-Object { [int]$_ })
    if ($parts[2] -ge 65535) { throw 'The Windows patch range is exhausted. Choose the next minor or major version explicitly.' }
    $parts[2]++
    $parts -join '.'
}

function Get-ReleaseBranchName([string]$Version) {
    ConvertTo-ReleaseSemVer $Version | Out-Null
    $parts = $Version.Split('.')
    "release/$($parts[0]).$($parts[1])"
}

function Assert-ReleaseSourceBranch([string]$Branch, [string]$Version) {
    ConvertTo-ReleaseSemVer $Version | Out-Null
    if ($Branch -ceq 'main') { return }
    $expected = Get-ReleaseBranchName $Version
    if ($Branch -cne $expected) { throw "Version $Version requires main or the release-line branch $expected." }
}

function Resolve-ReleaseType($State, [string]$Branch, [string]$RequestedType = 'Branch default') {
    Assert-ReleaseSourceBranch $Branch $State.Version
    $type = if (-not $RequestedType -or $RequestedType -ceq 'Branch default') { $State.ReleaseType } else { $RequestedType }
    if ($Branch -ceq 'main') {
        if ($State.Channel -cne 'Canary' -or $type -cne 'Canary') { throw 'Main produces Canary releases.' }
    } elseif ($State.Channel -cne 'Release' -or $type -cnotin @('Beta', 'Stable')) { throw 'A major.minor release branch produces Beta or Stable releases with a neutral Release source channel.' }
    return $type
}

function Get-MainReleaseLineDecision($State, [string]$ReleaseLineVersion) {
    ConvertTo-ReleaseSemVer $State.Version | Out-Null
    ConvertTo-ReleaseSemVer $ReleaseLineVersion | Out-Null
    $current = [version]$State.Version
    $release = [version]$ReleaseLineVersion
    if ($current.Major -gt $release.Major -or ($current.Major -eq $release.Major -and $current.Minor -gt $release.Minor)) {
        return [pscustomobject]@{ Bump = $false; Reason = 'Main is already on a later release line.' }
    }
    if ($current.Major -lt $release.Major) { throw 'Advance main to the new major explicitly before creating its release branch. Automation never increases a major version.' }
    if ($release.Minor -ge 65535) { throw 'The Windows minor range is exhausted. Choose a new major version explicitly.' }
    [pscustomobject]@{ Bump = $true; Version = "$($release.Major).$($release.Minor + 1).0"; Channel = 'Canary'; ReleaseType = 'Canary' }
}

function Get-PostReleaseDecision($State, [string]$PublishedVersion, [string]$Branch = 'main', [string]$PublishedType = 'Beta') {
    ConvertTo-ReleaseSemVer $PublishedVersion | Out-Null
    Assert-ReleaseSourceBranch $Branch $PublishedVersion
    $comparison = ([version]$State.Version).CompareTo([version]$PublishedVersion)
    if ($Branch -ceq 'main') {
        if ($comparison -gt 0) { return [pscustomobject]@{ Bump = $false; Reason = 'Main already has a newer version.' } }
        return [pscustomobject]@{ Bump = $true; Version = (Get-NextReleaseVersion $PublishedVersion); Channel = 'Canary'; ReleaseType = 'Canary' }
    }
    Assert-ReleaseSourceBranch $Branch $State.Version
    $nextType = if ($State.ReleaseType -ceq 'Stable' -or $PublishedType -ceq 'Stable') { 'Stable' } else { 'Beta' }
    if ($comparison -lt 0) { throw 'Release branch version is older than the published release; inspect it before advancing.' }
    if ($comparison -gt 0) {
        if ($State.Channel -ceq 'Release' -and $State.ReleaseType -ceq $nextType) { return [pscustomobject]@{ Bump = $false; Reason = 'Release branch already has a newer version on the correct track.' } }
        return [pscustomobject]@{ Bump = $true; Version = $State.Version; Channel = 'Release'; ReleaseType = $nextType }
    }
    $next = Get-NextReleaseVersion $PublishedVersion
    Assert-ReleaseSourceBranch $Branch $next
    [pscustomobject]@{ Bump = $true; Version = $next; Channel = 'Release'; ReleaseType = $nextType }
}

function Assert-CanaryReleaseLineAvailable([string]$Version, [string]$Repository, $Headers) {
    $branch = Get-ReleaseBranchName $Version
    $reserved = $null
    try { $reserved = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repository/git/ref/heads/$branch" -Headers $Headers } catch {
        if ([int]$_.Exception.Response.StatusCode -ne 404) { throw }
    }
    if ($reserved) { throw "Release line $branch is reserved. Advance main to its next minor version before releasing a Canary." }
}
