param()
$ErrorActionPreference = 'Stop'
$fixture = Join-Path ([System.IO.Path]::GetTempPath()) ('transmog-publication-test-' + [Guid]::NewGuid().ToString('N'))
$releaseRoot = Join-Path $fixture 'release'
$evidenceRoot = Join-Path $fixture 'evidence'
New-Item -ItemType Directory -Force $releaseRoot, $evidenceRoot | Out-Null
$installerName = 'Transmog_0.1.0_x64-setup.exe'
'fixture, never executable' | Set-Content -LiteralPath (Join-Path $releaseRoot $installerName)
'standalone CLI fixture' | Set-Content -LiteralPath (Join-Path $releaseRoot 'transmog-cli.exe')
'symbols archive fixture' | Set-Content -LiteralPath (Join-Path $releaseRoot 'Transmog_0.1.0_windows-x64-symbols.zip')
$cliHash = (Get-FileHash -LiteralPath (Join-Path $releaseRoot 'transmog-cli.exe')).Hash
$hash = (Get-FileHash -LiteralPath (Join-Path $releaseRoot $installerName)).Hash
'fixture signature' | Set-Content -LiteralPath (Join-Path $releaseRoot "$installerName.sig")
[ordered]@{ version = '0.1.0'; platforms = @{ 'windows-x86_64-nsis' = @{ url = "https://github.com/erik-anderson/transmog/releases/download/v0.1.0/$installerName"; signature = 'fixture signature' } } } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $releaseRoot 'latest.json')
$files = @(Get-ChildItem -LiteralPath $releaseRoot -File | ForEach-Object { @{ Name = $_.Name; Sha256 = (Get-FileHash -LiteralPath $_.FullName).Hash } })
[ordered]@{ Commit = 'fixture-commit'; RunId = '123'; SourceBranch = 'release/0.1'; Version = '0.1.0'; Channel = 'Release'; ReleaseType = 'Beta'; Installer = $installerName; Cli = 'transmog-cli.exe'; Symbols = 'Transmog_0.1.0_windows-x64-symbols.zip'; Files = $files } |
    ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $releaseRoot 'release-manifest.json')
[ordered]@{ Commit = 'fixture-commit'; RunId = '123'; InstallVerified = $true; UninstallVerified = $true; CliRuntimeVerified = $true; UpdaterSignatureVerified = $true; InstallerSha256 = $hash } |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $evidenceRoot 'installer-test.json')
[ordered]@{ Commit = 'fixture-commit'; RunId = '123'; UnprotectedJobAuthenticationDenied = $true } |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $evidenceRoot 'identity-permission-test.json')
'{}' | Set-Content -LiteralPath (Join-Path $evidenceRoot 'provenance.sigstore.json')
$names = @('GITHUB_SHA', 'GITHUB_RUN_ID', 'GITHUB_REPOSITORY', 'GITHUB_REF', 'GITHUB_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN')
$prior = @{}
foreach ($name in $names) { $prior[$name] = [Environment]::GetEnvironmentVariable($name) }
$cases = @(
    @{ Name = 'a published matching release'; Releases = @([pscustomobject]@{ tag_name = 'v0.1.0'; draft = $false; id = 1 }) },
    @{ Name = 'a published match alongside another draft'; Releases = @([pscustomobject]@{ tag_name = 'v0.1.0'; draft = $false; id = 1 }, [pscustomobject]@{ tag_name = 'v0.2.0'; draft = $true; id = 2 }) },
    @{ Name = 'ambiguous matching drafts'; Releases = @([pscustomobject]@{ tag_name = 'v0.1.0'; draft = $true; id = 1 }, [pscustomobject]@{ tag_name = 'v0.1.0'; draft = $true; id = 2 }) },
    @{ Name = 'a published match on the second page'; Releases = @(1..100 | ForEach-Object { [pscustomobject]@{ tag_name = "v2.0.$_"; draft = $true; id = $_ } }); SecondPage = @([pscustomobject]@{ tag_name = 'v0.1.0'; draft = $false; id = 101 }) }
)
try {
    $env:GITHUB_SHA = 'fixture-commit'
    $env:GITHUB_RUN_ID = '123'
    $env:GITHUB_REPOSITORY = 'erik-anderson/transmog'
    $env:GITHUB_REF = 'refs/heads/release/0.1'
    $env:GITHUB_TOKEN = 'fixture'
    $env:ACTIONS_ID_TOKEN_REQUEST_TOKEN = $null
    function Invoke-RestMethod {
        param($Uri, $Headers, $Method, $ContentType, $Body, $InFile)
        if ($Method -or $Uri -notlike '*/releases?per_page=100&page=*') { throw 'A publication mutation was attempted by the fixture.' }
        # Invoke-RestMethod emits a JSON array as one pipeline object.
        $response = if ($Uri -like '*page=2') { $secondPageFixtures } else { $releaseFixtures }
        Write-Output -NoEnumerate $response
    }
    foreach ($case in $cases) {
        $releaseFixtures = $case.Releases
        $secondPageFixtures = $case.SecondPage
        $rejected = $false
        try { & (Join-Path $PSScriptRoot 'publish-windows-draft.ps1') -ReleaseRoot $releaseRoot -EvidenceRoot $evidenceRoot } catch {
            if ($_.Exception.Message -notlike 'Refusing to overwrite a published*') { throw }
            $rejected = $true
        }
        if (-not $rejected) { throw "Publication accepted $($case.Name)." }
        Write-Host "Rejected $($case.Name) before any publication mutation."
    }
    function Invoke-RestMethod {
        param($Uri, $Headers, $Method, $ContentType, $Body, $InFile)
        if (-not $Method -and $Uri -like '*/git/ref/*') {
            throw [Microsoft.PowerShell.Commands.HttpResponseException]::new('No reserved fixture major', [Net.Http.HttpResponseMessage]::new([Net.HttpStatusCode]::NotFound))
        }
        if (-not $Method -and $Uri -like '*/releases?per_page=100&page=1') { Write-Output -NoEnumerate @(); return }
        if ($Method -ceq 'Post' -and $Uri -like '*/releases') {
            $publicationFixture.Body = $Body | ConvertFrom-Json
            if (-not $publicationFixture.Body.draft -or $publicationFixture.Body.target_commitish -cne 'fixture-commit') { throw 'Draft or source pin was lost.' }
            return [pscustomobject]@{ id=10; draft=$true; assets=@(); upload_url='https://uploads.github.com/fixture{?name}'; html_url='https://github.com/fixture/draft' }
        }
        if ($Method -ceq 'Post' -and $InFile) { $publicationFixture.Uploads++; return }
        if (-not $Method -and $Uri -like '*/releases/10') { return [pscustomobject]@{ draft=$true; assets=@(1..$publicationFixture.Uploads); html_url='https://github.com/fixture/draft' } }
        throw 'Unexpected publication fixture request.'
    }
    $priorSummary = $env:GITHUB_STEP_SUMMARY
    $env:GITHUB_STEP_SUMMARY = $null
    try {
        foreach ($type in @('Beta', 'Stable', 'Canary')) {
            $candidate = Get-Content -Raw -LiteralPath (Join-Path $releaseRoot 'release-manifest.json') | ConvertFrom-Json
            $candidate.ReleaseType = $type
            $candidate.Channel = if ($type -ceq 'Canary') { 'Canary' } else { 'Release' }
            $candidate.SourceBranch = if ($type -ceq 'Canary') { 'main' } else { 'release/0.1' }
            $candidate | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $releaseRoot 'release-manifest.json')
            $env:GITHUB_REF = "refs/heads/$($candidate.SourceBranch)"
            $publicationFixture = [pscustomobject]@{ Body=$null; Uploads=0 }
            & (Join-Path $PSScriptRoot 'publish-windows-draft.ps1') -ReleaseRoot $releaseRoot -EvidenceRoot $evidenceRoot
            if ($publicationFixture.Body.prerelease -ne ($type -cne 'Stable') -or $publicationFixture.Uploads -ne 9) { throw 'Wrong release track flags or asset count.' }
        }
        Write-Host 'Beta, Stable, and Canary publication retain drafts, source pins, assets, and expected prerelease flags.'
    } finally { $env:GITHUB_STEP_SUMMARY = $priorSummary }
} finally {
    foreach ($name in $names) {
        if ($null -eq $prior[$name]) {
            Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
        } else {
            Set-Item -LiteralPath "Env:$name" -Value $prior[$name]
        }
    }
}
