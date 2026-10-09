param([string]$Repository = 'erik-anderson/transmog')
. (Join-Path $PSScriptRoot 'release-version-common.ps1')
$headers = @{ Authorization = "Bearer $env:GITHUB_TOKEN"; Accept = 'application/vnd.github+json'; 'X-GitHub-Api-Version' = '2022-11-28'; 'User-Agent' = 'Transmog-stable-updates' }
$api = "https://api.github.com/repos/$Repository"
$best = $null
for ($page = 1; ; $page++) {
    $releases = Invoke-RestMethod -Uri "$api/releases?per_page=100&page=$page" -Headers $headers
    foreach ($release in $releases) {
        if ($release.draft -or $release.prerelease -or $release.tag_name -cnotmatch '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') { continue }
        $version = $release.tag_name.Substring(1)
        ConvertTo-ReleaseSemVer $version | Out-Null
        if (@($release.assets | Where-Object name -CEQ 'latest.json').Count -ne 1 -or
            @($release.assets | Where-Object name -CEQ "Transmog_${version}_x64-setup.exe.sig").Count -ne 1) { continue }
        if (-not $best -or [version]$version -gt [version]$best.tag_name.Substring(1)) { $best = $release }
    }
    if ($releases.Count -lt 100) { break }
}
if (-not $best) { Write-Host 'No stable release with signed update assets exists yet.'; return }
$latest = $null
try { $latest = Invoke-RestMethod -Uri "$api/releases/latest" -Headers $headers } catch {
    if ([int]$_.Exception.Response.StatusCode -ne 404) { throw }
}
if ($latest.id -eq $best.id) { Write-Host "Stable update feed already selects $($best.tag_name)."; return }
Invoke-RestMethod -Method Patch -Uri "$api/releases/$($best.id)" -Headers $headers -ContentType 'application/json' -Body '{"make_latest":"true"}' | Out-Null
Write-Host "Stable update feed now selects $($best.tag_name)."
