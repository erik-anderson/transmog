[CmdletBinding(DefaultParameterSetName = 'Set')]
param(
    [Parameter(Mandatory, ParameterSetName = 'Set')][string]$Version,
    [Parameter(Mandatory, ParameterSetName = 'Check')][switch]$Check,
    [Parameter(ParameterSetName = 'Set')][ValidateSet('Canary', 'Release')][string]$Channel,
    [Parameter(ParameterSetName = 'Set')][ValidateSet('Canary', 'Beta', 'Stable')][string]$ReleaseType,
    [string]$RepositoryRoot = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
. (Join-Path $PSScriptRoot 'release-version-common.ps1')
$repositoryRoot = [IO.Path]::GetFullPath($RepositoryRoot)
$state = Get-ReleaseVersionState $repositoryRoot
$selectedVersion = if ($Check) { $state.Version } else { $Version }
$newSemVer = ConvertTo-ReleaseSemVer $selectedVersion
$selectedType = if ($ReleaseType) { $ReleaseType } elseif ($Channel -ceq 'Canary') { 'Canary' } elseif ($Channel -ceq 'Release' -and $state.ReleaseType -ceq 'Canary') { 'Beta' } else { $state.ReleaseType }
$selectedChannel = if ($selectedType -ceq 'Canary') { 'Canary' } else { 'Release' }
if ($Channel -and $Channel -cne $selectedChannel) { throw 'Source channel and release track disagree.' }
$metadata = cargo metadata --manifest-path (Join-Path $repositoryRoot 'Cargo.toml') --no-deps --format-version 1 --locked --offline | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Cannot read the locked Cargo workspace.' }
$currentVersion = [string]$metadata.packages[0].version
if (@($metadata.packages | Where-Object version -CNE $currentVersion).Count) { throw 'Workspace package versions differ.' }
if ($currentVersion -cne $state.SemVer) { throw 'Cargo differs from release-version.json.' }
$currentRequirement = $currentVersion.Split('+')[0]
$newRequirement = $newSemVer.Split('+')[0]

$configPath = Join-Path $repositoryRoot 'apps/desktop/tauri.conf.json'
$uiPath = Join-Path $repositoryRoot 'apps/desktop/ui/package.json'
$uiLockPath = Join-Path $repositoryRoot 'apps/desktop/ui/package-lock.json'
$uiLock = Get-Content -Raw -LiteralPath $uiLockPath | ConvertFrom-Json -AsHashtable
foreach ($value in @(
    (Get-Content -Raw -LiteralPath $configPath | ConvertFrom-Json).version,
    (Get-Content -Raw -LiteralPath $uiPath | ConvertFrom-Json).version,
    $uiLock.version,
    $uiLock.packages[''].version
)) {
    if ($value -cne $currentVersion) { throw 'Tauri, Cargo, and desktop UI versions must agree before changing the release version.' }
}

$manifestPaths = @((Join-Path $repositoryRoot 'Cargo.toml')) + @($metadata.packages.manifest_path) + @((Join-Path $repositoryRoot 'fuzz/Cargo.toml'))
$pinPattern = '(?m)(?<prefix>^transmog[\w-]*\s*=\s*\{(?=[^\r\n}]*\bpath\s*=)[^\r\n}]*\bversion\s*=\s*")(?<value>[^"]+)(?<suffix>")'
$original = @{}
foreach ($path in $manifestPaths + @($configPath, $uiPath, $uiLockPath, (Join-Path $repositoryRoot 'release-version.json'), (Join-Path $repositoryRoot 'Cargo.lock'), (Join-Path $repositoryRoot 'fuzz/Cargo.lock'))) {
    if (-not ([IO.Path]::GetFullPath($path)).StartsWith($repositoryRoot.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'A version file is outside the workspace.' }
    $original[$path] = [IO.File]::ReadAllText($path)
}
foreach ($path in $manifestPaths) {
    foreach ($match in [regex]::Matches($original[$path], $pinPattern)) {
        if ($match.Groups['value'].Value -cne "=$currentRequirement") { throw "An internal dependency version differs in $path" }
    }
}
if ($Check -or ($newSemVer -ceq $currentVersion -and $selectedChannel -ceq $state.Channel -and $selectedType -ceq $state.ReleaseType)) {
    Write-Host "Committed release version: $($state.Version) ($($state.ReleaseType))"
    return
}
$statePath = Join-Path $repositoryRoot 'release-version.json'
$stateText = ([ordered]@{ version = $selectedVersion; channel = $selectedChannel; releaseType = $selectedType } | ConvertTo-Json) + "`n"
if ($newSemVer -ceq $currentVersion) {
    [IO.File]::WriteAllText($statePath, $stateText, [Text.UTF8Encoding]::new($false))
    Write-Host "Set release track to $selectedType at $selectedVersion."
    return
}

function Replace-Version([string]$Text, [string]$Pattern) {
    if ([regex]::Matches($Text, $Pattern).Count -ne 1) { throw 'Expected exactly one version field in the current file format.' }
    return [regex]::Replace($Text, $Pattern, [System.Text.RegularExpressions.MatchEvaluator]{ param($match)
        if ($match.Groups['value'].Value -cne $currentVersion) { throw 'Unexpected version field value.' }
        $match.Groups['prefix'].Value + $newSemVer + $match.Groups['suffix'].Value
    })
}
$updated = @{}
foreach ($path in $manifestPaths) {
    $updated[$path] = [regex]::Replace($original[$path], $pinPattern, [System.Text.RegularExpressions.MatchEvaluator]{ param($match)
        $match.Groups['prefix'].Value + "=$newRequirement" + $match.Groups['suffix'].Value
    })
}
$rootManifest = Join-Path $repositoryRoot 'Cargo.toml'
$updated[$rootManifest] = Replace-Version $updated[$rootManifest] '(?ms)(?<prefix>^\[workspace\.package\]\r?\n(?:(?!^\[).)*?^version\s*=\s*")(?<value>[^"]+)(?<suffix>")'
foreach ($path in @($configPath, $uiPath, $uiLockPath)) {
    $updated[$path] = Replace-Version $original[$path] '(?m)(?<prefix>^  "version": ")(?<value>[^"]+)(?<suffix>")'
}
$updated[$uiLockPath] = Replace-Version $updated[$uiLockPath] '(?ms)(?<prefix>^    "": \{\r?\n(?:(?!^    \}).)*?^      "version": ")(?<value>[^"]+)(?<suffix>")'
$updated[$statePath] = $stateText
$utf8 = [System.Text.UTF8Encoding]::new($false)
try {
    foreach ($path in $updated.Keys) { [IO.File]::WriteAllText($path, $updated[$path], $utf8) }
    foreach ($manifestPath in @($rootManifest, (Join-Path $repositoryRoot 'fuzz/Cargo.toml'))) {
        cargo update --manifest-path $manifestPath --workspace --offline
        if ($LASTEXITCODE -ne 0) { throw "Could not refresh the lockfile for $manifestPath" }
    }
} catch {
    # Restore the exact pre-command contents, including any maintainer edits.
    foreach ($path in $original.Keys) { [IO.File]::WriteAllText($path, $original[$path], $utf8) }
    throw
}
Write-Host "Set release version to $selectedVersion ($selectedType). Review git diff, then commit and push before running the release workflow."
