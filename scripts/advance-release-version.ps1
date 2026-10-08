param()
. (Join-Path $PSScriptRoot 'release-version-common.ps1')
$PSNativeCommandUseErrorActionPreference = $true
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') { throw 'Version maintenance runs only in the dedicated hosted workflow.' }
$event = Get-Content -Raw -LiteralPath $env:GITHUB_EVENT_PATH | ConvertFrom-Json
$creatingBranch = $env:GITHUB_EVENT_NAME -ceq 'create'
$temporary = Join-Path $env:RUNNER_TEMP ('transmog-version-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temporary | Out-Null
# Keep the implementation from main while switching between branches.
foreach ($name in @('set-release-version.ps1', 'release-version-common.ps1')) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination (Join-Path $temporary $name)
}
if ($creatingBranch) {
    if ($event.ref_type -cne 'branch' -or $event.ref -cnotmatch '^release/(0|[1-9][0-9]*)$') { Write-Host 'Not a per-major release branch.'; return }
    $sourceBranch = $event.ref
    $releaseMajor = [int]$sourceBranch.Substring('release/'.Length)
    ConvertTo-ReleaseSemVer "$releaseMajor.0.0.0" | Out-Null
    $description = "creation of $sourceBranch"
} else {
    $release = $event.release
    if ($release.draft -or $release.tag_name -cnotmatch '^v\d+\.\d+\.\d+\.\d+$') { Write-Host 'No version update for a draft or legacy three-part release.'; return }
    $publishedVersion = $release.tag_name.Substring(1)
    ConvertTo-ReleaseSemVer $publishedVersion | Out-Null
    $assets = @($release.assets | Where-Object name -CEQ 'release-manifest.json')
    if ($assets.Count -ne 1) { throw 'The published release must contain one release-manifest.json.' }
    $headers = @{ Authorization = "Bearer $env:GITHUB_TOKEN"; Accept = 'application/octet-stream'; 'X-GitHub-Api-Version' = '2022-11-28'; 'User-Agent' = 'Transmog-release-version' }
    $manifestPath = Join-Path $temporary 'release-manifest.json'
    Invoke-WebRequest -Uri "https://api.github.com/repos/$env:GITHUB_REPOSITORY/releases/assets/$($assets[0].id)" -Headers $headers -OutFile $manifestPath
    if ($assets[0].digest -and $assets[0].digest -cne ('sha256:' + (Get-FileHash -LiteralPath $manifestPath).Hash.ToLowerInvariant())) { throw 'Release manifest digest differs from GitHub.' }
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    if ($manifest.Version -cne $publishedVersion -or $manifest.Commit -cnotmatch '^[a-f0-9]{40}$') { throw 'Release tag or commit is invalid.' }
    Resolve-ReleaseType $manifest $manifest.SourceBranch $manifest.ReleaseType | Out-Null
    $sourceBranch = $manifest.SourceBranch
    $releaseMajor = [int]$publishedVersion.Split('.')[0]
    $publishedType = if ($sourceBranch -ceq 'main') { 'Canary' } elseif ($release.prerelease) { 'Beta' } else { 'Stable' }
    $description = $release.tag_name
    git fetch --no-tags origin "refs/tags/$($release.tag_name)"
    if ($LASTEXITCODE -ne 0) { throw 'Could not fetch the published tag.' }
    if ((git rev-parse 'FETCH_HEAD^{commit}') -cne $manifest.Commit) { throw 'Published tag differs from installer source commit.' }
}
git config user.name 'github-actions[bot]'
git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
$repositoryRoot = (Get-Location).Path

function Update-BranchAfterEvent([string]$TargetBranch) {
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        $priorNativePreference = $PSNativeCommandUseErrorActionPreference
        try {
            $PSNativeCommandUseErrorActionPreference = $false
            $fetchOutput = git fetch --no-tags origin "refs/heads/${TargetBranch}:refs/remotes/origin/$TargetBranch" 2>&1 | Out-String
            $fetchExit = $LASTEXITCODE
        } finally { $PSNativeCommandUseErrorActionPreference = $priorNativePreference }
        if ($fetchExit -ne 0) {
            if ($fetchOutput -like '*couldn''t find remote ref*') { Write-Host "$TargetBranch was deleted; skipping its update."; $global:LASTEXITCODE = 0; return }
            throw "Could not fetch $TargetBranch : $fetchOutput"
        }
        git checkout -B prepare-next-version "refs/remotes/origin/$TargetBranch"
        if ($LASTEXITCODE -ne 0 -or (git status --porcelain)) { throw 'The hosted version checkout is not clean.' }
        $state = Get-ReleaseVersionState $repositoryRoot
        if ($TargetBranch -cne 'main') {
            Assert-ReleaseSourceBranch $TargetBranch $state.Version
            if (-not $creatingBranch) {
                git merge-base --is-ancestor $manifest.Commit HEAD
                if ($LASTEXITCODE -ne 0) { throw 'Release source commit is not an ancestor of its branch.' }
            }
        }
        if ($TargetBranch -ceq 'main' -and $sourceBranch -cne 'main') {
            $decision = Get-MainMajorDecision $state $releaseMajor
        } elseif ($creatingBranch) {
            $type = if ($state.ReleaseType -ceq 'Canary') { 'Beta' } else { $state.ReleaseType }
            $decision = [pscustomobject]@{ Bump = ($state.Channel -cne 'Release'); Version = $state.Version; Channel = 'Release'; ReleaseType = $type; Reason = 'Release branch already initialized.' }
        } else {
            $decision = Get-PostReleaseDecision $state $publishedVersion $TargetBranch $publishedType
        }
        if (-not $decision.Bump) { Write-Host "$TargetBranch : $($decision.Reason)"; return }
        # Only a numeric version change requires refreshing Cargo's locked graph.
        if ($decision.Version -cne $state.Version) {
            cargo fetch --locked
            cargo fetch --manifest-path fuzz/Cargo.toml --locked
        }
        & (Join-Path $temporary 'set-release-version.ps1') -RepositoryRoot $repositoryRoot -Version $decision.Version -ReleaseType $decision.ReleaseType
        git add --update
        git commit -m "Prepare $($decision.Version) $($decision.ReleaseType) on $TargetBranch after $description"
        if ($LASTEXITCODE -ne 0) { throw 'Could not commit the version update.' }
        try {
            $PSNativeCommandUseErrorActionPreference = $false
            git push origin "HEAD:refs/heads/$TargetBranch"
            $pushExit = $LASTEXITCODE
        } finally { $PSNativeCommandUseErrorActionPreference = $priorNativePreference }
        if ($pushExit -eq 0) {
            Write-Host "Updated $TargetBranch to $($decision.Version) $($decision.ReleaseType)."
            if ($env:GITHUB_STEP_SUMMARY) { "$description : **$TargetBranch** is now **$($decision.Version) $($decision.ReleaseType)**." | Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY }
            return
        }
        # Re-read concurrent changes; never force-push.
    }
    throw "Could not push $TargetBranch after three attempts; inspect branch protection or concurrent changes."
}
$failures = [System.Collections.Generic.List[string]]::new()
foreach ($branch in @($sourceBranch, 'main') | Select-Object -Unique) {
    try { Update-BranchAfterEvent $branch } catch { $failures.Add("$branch : $($_.Exception.Message)") }
}
if ($failures.Count) { throw ($failures -join "`n") }
