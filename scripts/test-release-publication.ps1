param()
$ErrorActionPreference = 'Stop'
$fixture = Join-Path ([System.IO.Path]::GetTempPath()) ('transmog-publication-test-' + [Guid]::NewGuid().ToString('N'))
$releaseRoot = Join-Path $fixture 'release'
$evidenceRoot = Join-Path $fixture 'evidence'
New-Item -ItemType Directory -Force $releaseRoot, $evidenceRoot | Out-Null
$installerName = 'Transmog_0.1.0_x64-setup.exe'
'fixture, never executable' | Set-Content -LiteralPath (Join-Path $releaseRoot $installerName)
$hash = (Get-FileHash -LiteralPath (Join-Path $releaseRoot $installerName)).Hash
[ordered]@{ Commit = 'fixture-commit'; RunId = '123'; Version = '0.1.0'; Installer = $installerName; Files = @(@{ Name = $installerName; Sha256 = $hash }) } |
    ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $releaseRoot 'release-manifest.json')
[ordered]@{ Commit = 'fixture-commit'; RunId = '123'; InstallVerified = $true; UninstallVerified = $true; InstallerSha256 = $hash } |
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
    @{ Name = 'ambiguous matching drafts'; Releases = @([pscustomobject]@{ tag_name = 'v0.1.0'; draft = $true; id = 1 }, [pscustomobject]@{ tag_name = 'v0.1.0'; draft = $true; id = 2 }) }
)
try {
    $env:GITHUB_SHA = 'fixture-commit'
    $env:GITHUB_RUN_ID = '123'
    $env:GITHUB_REPOSITORY = 'erik-anderson/transmog'
    $env:GITHUB_REF = 'refs/heads/main'
    $env:GITHUB_TOKEN = 'fixture'
    $env:ACTIONS_ID_TOKEN_REQUEST_TOKEN = $null
    function Invoke-RestMethod {
        param($Uri, $Headers, $Method, $ContentType, $Body, $InFile)
        if ($Method -or $Uri -notlike '*/releases?per_page=100') { throw 'A publication mutation was attempted by the fixture.' }
        # Invoke-RestMethod emits a JSON array as one pipeline object.
        Write-Output -NoEnumerate $releaseFixtures
    }
    foreach ($case in $cases) {
        $releaseFixtures = $case.Releases
        $rejected = $false
        try { & (Join-Path $PSScriptRoot 'publish-windows-draft.ps1') -ReleaseRoot $releaseRoot -EvidenceRoot $evidenceRoot } catch {
            if ($_.Exception.Message -notlike 'Refusing to overwrite a published*') { throw }
            $rejected = $true
        }
        if (-not $rejected) { throw "Publication accepted $($case.Name)." }
        Write-Host "Rejected $($case.Name) before any publication mutation."
    }
} finally {
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $prior[$name]) }
}
