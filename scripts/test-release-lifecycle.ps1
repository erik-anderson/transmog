$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$taskRoot = Split-Path -Parent $PSScriptRoot
$fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) ('transmog-lifecycle-' + [Guid]::NewGuid().ToString('N'))
$checkout = Join-Path $fixtureRoot 'checkout'
$origin = Join-Path $fixtureRoot 'origin.git'
$runnerTemp = Join-Path $fixtureRoot 'runner-temp'
New-Item -ItemType Directory -Force $checkout, $runnerTemp | Out-Null
git init --bare -q $origin
git init --initial-branch=main -q $checkout
Push-Location $checkout
try {
    git config user.name Fixture
    git config user.email fixture@example.invalid
    git remote add origin $origin
    foreach ($folder in @('apps/desktop/src','apps/desktop/ui','fuzz/src')) { New-Item -ItemType Directory -Force $folder | Out-Null }
    @'
[workspace]
members = ["apps/desktop"]
resolver = "2"
[workspace.package]
version = "0.1.0"
edition = "2024"
'@ | Set-Content Cargo.toml
    @'
[package]
name = "transmog-desktop"
version.workspace = true
edition.workspace = true
'@ | Set-Content apps/desktop/Cargo.toml
    '// Metadata fixture.' | Set-Content apps/desktop/src/lib.rs
    @'
[package]
name = "transmog-fuzz-fixture"
version = "0.0.0"
edition = "2024"
[workspace]
'@ | Set-Content fuzz/Cargo.toml
    '// Metadata fixture.' | Set-Content fuzz/src/lib.rs
    [ordered]@{ version='0.1.0';channel='Canary';releaseType='Canary' } | ConvertTo-Json | Set-Content release-version.json
    [ordered]@{ version='0.1.0' } | ConvertTo-Json | Set-Content apps/desktop/tauri.conf.json
    [ordered]@{ name='@transmog/desktop-ui';version='0.1.0';private=$true } | ConvertTo-Json | Set-Content apps/desktop/ui/package.json
    [ordered]@{ name='@transmog/desktop-ui';version='0.1.0';lockfileVersion=3;packages=[ordered]@{ ''=[ordered]@{name='@transmog/desktop-ui';version='0.1.0'} } } | ConvertTo-Json -Depth 5 | Set-Content apps/desktop/ui/package-lock.json
    cargo generate-lockfile --offline
    cargo generate-lockfile --manifest-path fuzz/Cargo.toml --offline
    git add .
    git commit -q -m 'Canary main 0.1.0'
    git branch release/0.1
    git push -q origin main release/0.1
    $names = @('GITHUB_ACTIONS','GITHUB_EVENT_NAME','RUNNER_ENVIRONMENT','RUNNER_TEMP','GITHUB_EVENT_PATH','GITHUB_REPOSITORY','GITHUB_TOKEN','GITHUB_STEP_SUMMARY','CARGO_NET_OFFLINE')
    $prior = @{}
    foreach ($name in $names) { $prior[$name] = [Environment]::GetEnvironmentVariable($name) }
    $fixtureDownload = [pscustomobject]@{ Path=(Join-Path $fixtureRoot 'manifest.json') }
    function Invoke-WebRequest { param($Uri,$Headers,$OutFile)
        if ($Uri -cne 'https://api.github.com/repos/erik-anderson/transmog/releases/assets/1') { throw 'Unexpected external request.' }
        Copy-Item -LiteralPath $fixtureDownload.Path -Destination $OutFile
    }
    function Invoke-Event([string]$Name, $Data) {
        $env:GITHUB_EVENT_NAME = $Name
        $Data | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $env:GITHUB_EVENT_PATH
        & (Join-Path $taskRoot 'scripts/advance-release-version.ps1')
    }
    function Assert-Remote([string]$Branch,[string]$Version,[string]$Type) {
        $actual = git show "refs/remotes/origin/${Branch}:release-version.json" | ConvertFrom-Json
        $channel = if ($Type -ceq 'Canary') { 'Canary' } else { 'Release' }
        if ($actual.version -cne $Version -or $actual.releaseType -cne $Type -or $actual.channel -cne $channel) { throw "Unexpected version/track on $Branch" }
    }
    function Publish-Fixture([string]$Branch,[string]$Version,[bool]$Prerelease) {
        $tag = "v$Version"
        if (-not (git tag --list $tag)) {
            git tag $tag "refs/remotes/origin/$Branch"
            git push -q origin "refs/tags/$tag"
        }
        $commit = git rev-parse "refs/tags/$tag"
        $tagState = git show "refs/tags/${tag}:release-version.json" | ConvertFrom-Json
        [ordered]@{ Version=$Version;Channel=$tagState.channel;ReleaseType=$tagState.releaseType;SourceBranch=$Branch;Commit=$commit } | ConvertTo-Json | Set-Content -LiteralPath $fixtureDownload.Path
        $digest = 'sha256:' + (Get-FileHash -LiteralPath $fixtureDownload.Path).Hash.ToLowerInvariant()
        Invoke-Event release @{release=@{tag_name=$tag;draft=$false;prerelease=$Prerelease;assets=@(@{name='release-manifest.json';id=1;digest=$digest})}}
    }
    try {
        $env:GITHUB_ACTIONS='true'
        $env:RUNNER_ENVIRONMENT='github-hosted'
        $env:RUNNER_TEMP=$runnerTemp
        $env:GITHUB_EVENT_PATH=Join-Path $fixtureRoot event.json
        $env:GITHUB_REPOSITORY='erik-anderson/transmog'
        $env:GITHUB_TOKEN='fixture'
        $env:GITHUB_STEP_SUMMARY=$null
        $env:CARGO_NET_OFFLINE='true'
        Invoke-Event create @{ref_type='branch';ref='release/0.1'}
        Assert-Remote release/0.1 0.1.0 Beta
        Assert-Remote main 0.2.0 Canary
        $mainReserved = git rev-parse refs/remotes/origin/main
        Invoke-Event create @{ref_type='branch';ref='release/0.1'}
        if ((git rev-parse refs/remotes/origin/main) -cne $mainReserved) { throw 'Repeated creation advanced main twice.' }
        Publish-Fixture release/0.1 0.1.0 $true
        Assert-Remote release/0.1 0.1.1 Beta
        Assert-Remote main 0.2.0 Canary
        $betaCommit = git rev-parse refs/remotes/origin/release/0.1
        Publish-Fixture release/0.1 0.1.0 $true
        if ((git rev-parse refs/remotes/origin/release/0.1) -cne $betaCommit) { throw 'Repeated Beta publication advanced twice.' }
        Publish-Fixture release/0.1 0.1.0 $false
        Assert-Remote release/0.1 0.1.1 Stable
        $stableCommit = git rev-parse refs/remotes/origin/release/0.1
        Publish-Fixture release/0.1 0.1.0 $false
        Publish-Fixture release/0.1 0.1.0 $true
        if ((git rev-parse refs/remotes/origin/release/0.1) -cne $stableCommit) { throw 'Promotion repeated or Stable track was reset to Beta.' }
        Publish-Fixture main 0.2.0 $true
        Assert-Remote main 0.2.1 Canary
        $canaryCommit = git rev-parse refs/remotes/origin/main
        Publish-Fixture main 0.2.0 $true
        if ((git rev-parse refs/remotes/origin/main) -cne $canaryCommit) { throw 'Repeated Canary publication advanced twice.' }
        Publish-Fixture main 0.2.1 $true
        Assert-Remote main 0.2.2 Canary
        git branch release/0.2 refs/remotes/origin/main
        git push -q origin release/0.2
        Invoke-Event create @{ref_type='branch';ref='release/0.2'}
        Assert-Remote release/0.2 0.2.2 Beta
        Assert-Remote main 0.3.0 Canary
        Publish-Fixture release/0.1 0.1.1 $false
        Assert-Remote release/0.1 0.1.2 Stable
        Assert-Remote main 0.3.0 Canary
        git push -q origin --delete release/0.1
        Publish-Fixture release/0.1 0.1.1 $false
        Assert-Remote main 0.3.0 Canary
        Write-Host 'Release lifecycle passed: branch creation reserves a minor line; Beta advances patches; promotion persists Stable; Canary increments only patch; older-line hotfixes and repeat events preserve main.'
    } finally {
        foreach ($name in $names) {
            if ($null -eq $prior[$name]) {
                Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
            } else {
                Set-Item -LiteralPath "Env:$name" -Value $prior[$name]
            }
        }
    }
} finally { Pop-Location }
