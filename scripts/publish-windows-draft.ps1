param([Parameter(Mandatory)][string]$ReleaseRoot, [Parameter(Mandatory)][string]$EvidenceRoot)
. (Join-Path $PSScriptRoot 'windows-release-common.ps1')
$manifest = Assert-SignedRelease -ReleaseRoot $ReleaseRoot -Commit $env:GITHUB_SHA -RunId $env:GITHUB_RUN_ID
if ($env:ACTIONS_ID_TOKEN_REQUEST_TOKEN) { throw 'Publication must not have Azure OIDC permission.' }
$installerTest = Get-Content -Raw -LiteralPath (Join-Path $EvidenceRoot 'installer-test.json') | ConvertFrom-Json
$identityTest = Get-Content -Raw -LiteralPath (Join-Path $EvidenceRoot 'identity-permission-test.json') | ConvertFrom-Json
foreach ($evidence in @($installerTest, $identityTest)) {
    if ($evidence.Commit -cne $env:GITHUB_SHA -or $evidence.RunId -cne $env:GITHUB_RUN_ID) { throw 'Qualification evidence is from a different run.' }
}
if (-not $installerTest.InstallVerified -or -not $installerTest.UninstallVerified -or -not $identityTest.UnprotectedJobAuthenticationDenied) { throw 'Release gate evidence did not pass.' }
if (-not (Test-Path -LiteralPath (Join-Path $EvidenceRoot 'provenance.sigstore.json') -PathType Leaf)) { throw 'Missing verified provenance bundle.' }
$expectedInstallerHash = ($manifest.Files | Where-Object Name -CEQ $manifest.Installer).Sha256
if ($installerTest.InstallerSha256 -cne $expectedInstallerHash) { throw 'Qualification used a different installer.' }
if ($env:GITHUB_REPOSITORY -cne 'erik-anderson/transmog' -or $env:GITHUB_REF -cnotmatch '^refs/heads/(main|release/(0|[1-9][0-9]*))$' -or
    $manifest.SourceBranch -cne $env:GITHUB_REF.Substring('refs/heads/'.Length)) { throw 'Draft publication requires an approved source branch matching the manifest.' }
Assert-ReleaseSourceBranch $manifest.SourceBranch $manifest.Version
$headers = @{ Authorization = "Bearer $env:GITHUB_TOKEN"; Accept = 'application/vnd.github+json'; 'X-GitHub-Api-Version' = '2022-11-28'; 'User-Agent' = 'Transmog-draft-release' }
$api = "https://api.github.com/repos/$env:GITHUB_REPOSITORY"
$tag = 'v' + $manifest.Version
if ($manifest.SourceBranch -ceq 'main') { Assert-CanaryMajorAvailable $manifest.Version $env:GITHUB_REPOSITORY $headers }
$releases = [System.Collections.Generic.List[object]]::new()
for ($page = 1; ; $page++) {
    # Invoke-RestMethod returns a JSON array as one pipeline object; enumerate
    # it explicitly and inspect every page before mutating a draft.
    $pageReleases = Invoke-RestMethod -Uri "$api/releases?per_page=100&page=$page" -Headers $headers
    foreach ($item in $pageReleases) { $releases.Add($item) }
    if ($pageReleases.Count -lt 100) { break }
}
$existing = @($releases | Where-Object tag_name -CEQ $tag)
if ($existing.Count -gt 1 -or ($existing.Count -eq 1 -and -not $existing[0].draft)) { throw "Refusing to overwrite a published or ambiguous release: $tag" }
$body = @{
    tag_name = $tag; target_commitish = $env:GITHUB_SHA; name = "Transmog $tag" + $(if ($manifest.ReleaseType -cne 'Stable') { " $($manifest.ReleaseType)" }); draft = $true; prerelease = ($manifest.ReleaseType -cne 'Stable')
    body = "Signed Windows x64 NSIS installer and standalone transmog-cli.exe. Publisher: $($manifest.Publisher).`n`nBuild, Authenticode/timestamp checks, hosted installer smoke test, Azure permission denial tests, and provenance verification passed in [run $env:GITHUB_RUN_ID](https://github.com/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID). Source commit: $env:GITHUB_SHA.`n`nThe clean Windows 11 release checklist is deferred by the maintainer.`n`nBefore publishing, add release notes, review this installer, and choose whether this is a prerelease."
} | ConvertTo-Json
if ($existing.Count -eq 1) {
    $release = Invoke-RestMethod -Method Patch -Uri "$api/releases/$($existing[0].id)" -Headers $headers -ContentType 'application/json' -Body $body
} else {
    $release = Invoke-RestMethod -Method Post -Uri "$api/releases" -Headers $headers -ContentType 'application/json' -Body $body
}
if (-not $release.draft) { throw 'GitHub returned a non-draft release.' }
$assets = @(Get-ChildItem -LiteralPath $ReleaseRoot -File) + @(Get-ChildItem -LiteralPath $EvidenceRoot -File)
foreach ($asset in $assets) {
    foreach ($oldAsset in @($release.assets | Where-Object name -CEQ $asset.Name)) {
        Invoke-RestMethod -Method Delete -Uri "$api/releases/assets/$($oldAsset.id)" -Headers $headers | Out-Null
    }
    $uploadUrl = ($release.upload_url -split '\{')[0] + '?name=' + [Uri]::EscapeDataString($asset.Name)
    Invoke-RestMethod -Method Post -Uri $uploadUrl -Headers $headers -ContentType 'application/octet-stream' -InFile $asset.FullName | Out-Null
}
$verified = Invoke-RestMethod -Uri "$api/releases/$($release.id)" -Headers $headers
if (-not $verified.draft -or $verified.assets.Count -ne $assets.Count) { throw 'Draft release asset verification failed.' }
Write-Host "Draft release: $($verified.html_url)"
if ($env:GITHUB_STEP_SUMMARY) { "Created [signed draft $tag]($($verified.html_url)) from commit $env:GITHUB_SHA. Download and review the installer, edit the release notes, then publish the existing draft when ready. See [the release process](https://github.com/$env:GITHUB_REPOSITORY/blob/$env:GITHUB_SHA/docs/windows-release.md#manual-github-release-process)." | Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY }
