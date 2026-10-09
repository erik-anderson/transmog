param()
. (Join-Path $PSScriptRoot 'release-version-common.ps1')
function New-State([string]$Version, [string]$Type) {
    [pscustomobject]@{ Version = $Version; ReleaseType = $Type; Channel = $(if ($Type -ceq 'Canary') { 'Canary' } else { 'Release' }) }
}
foreach ($case in @(
    @{ Current = '0.0.1'; Type = 'Canary'; Branch = 'main'; Published = '0.0.1'; Next = '0.0.2'; NextType = 'Canary' },
    @{ Current = '1.2.7'; Type = 'Canary'; Branch = 'main'; Published = '1.2.7'; Next = '1.2.8'; NextType = 'Canary' },
    @{ Current = '1.2.8'; Type = 'Canary'; Branch = 'main'; Published = '1.2.7'; Next = $null },
    @{ Current = '1.2.10'; Type = 'Canary'; Branch = 'main'; Published = '1.2.9'; Next = $null },
    @{ Current = '1.2.7'; Type = 'Beta'; Branch = 'release/1.2'; Published = '1.2.7'; PublishedType = 'Beta'; Next = '1.2.8'; NextType = 'Beta' },
    @{ Current = '1.2.8'; Type = 'Beta'; Branch = 'release/1.2'; Published = '1.2.7'; PublishedType = 'Stable'; Next = '1.2.8'; NextType = 'Stable' },
    @{ Current = '1.2.8'; Type = 'Stable'; Branch = 'release/1.2'; Published = '1.2.7'; PublishedType = 'Stable'; Next = $null },
    @{ Current = '1.2.8'; Type = 'Stable'; Branch = 'release/1.2'; Published = '1.2.7'; PublishedType = 'Beta'; Next = $null }
)) {
    $decision = Get-PostReleaseDecision (New-State $case.Current $case.Type) $case.Published $case.Branch $case.PublishedType
    if ($decision.Bump -ne [bool]$case.Next -or ($case.Next -and ($decision.Version -cne $case.Next -or $decision.ReleaseType -cne $case.NextType))) { throw 'Incorrect publication version/track decision.' }
}
foreach ($case in @(
    @{ Current = '0.0.1'; Line = '0.0.0'; Next = '0.1.0' },
    @{ Current = '0.2.2'; Line = '0.2.0'; Next = '0.3.0' },
    @{ Current = '0.1.7'; Line = '0.2.0'; Next = '0.3.0' },
    @{ Current = '0.3.8'; Line = '0.2.0'; Next = $null },
    @{ Current = '1.0.0'; Line = '0.2.0'; Next = $null }
)) {
    $decision = Get-MainReleaseLineDecision (New-State $case.Current 'Canary') $case.Line
    if ($decision.Bump -ne [bool]$case.Next -or ($case.Next -and ($decision.Version -cne $case.Next -or $decision.ReleaseType -cne 'Canary'))) { throw 'Incorrect main release-line reservation decision.' }
}
foreach ($case in @(@{Current='0.9.3';Line='1.0.0'}, @{Current='0.65535.1';Line='0.65535.0'})) {
    $rejected = $false
    try { Get-MainReleaseLineDecision (New-State $case.Current 'Canary') $case.Line | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw 'Automation silently increased the major version.' }
}
foreach ($invalid in @('1.2', '1.2.3.4', '1.2.65536', '1.02.3', 'v1.2.3', '1.2.3+4')) {
    $rejected = $false
    try { ConvertTo-ReleaseSemVer $invalid | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw "Invalid version accepted: $invalid" }
}
Assert-ReleaseSourceBranch 'release/0.0' '0.0.1'
foreach ($branch in @('release/0', 'release/0.1', 'release/0.00', 'release/0.0.1')) {
    $rejected = $false
    try { Assert-ReleaseSourceBranch $branch '0.0.1' } catch { $rejected = $true }
    if (-not $rejected) { throw 'An invalid or mismatched release branch was accepted.' }
}
$rejected = $false
try { Get-PostReleaseDecision (New-State '1.2.6' 'Stable') '1.2.7' 'release/1.2' 'Stable' | Out-Null } catch { $rejected = $true }
if (-not $rejected) { throw 'A release branch reset to an older version was advanced.' }

$overflowRejected = $false
try { Get-NextReleaseVersion '1.2.65535' | Out-Null } catch { $overflowRejected = $true }
if (-not $overflowRejected) { throw 'Patch overflow silently advanced a minor version.' }

$fixture = Join-Path ([IO.Path]::GetTempPath()) ('transmog-release-version-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
$statePath = Join-Path $fixture 'release-version.json'
@{ version = '1.2.7'; channel = 'Release'; releaseType = 'Beta' } | ConvertTo-Json | Set-Content -LiteralPath $statePath
function Invoke-RestMethod {
    param($Uri, $Headers)
    $fixtureApi.Calls++
    $result = if ($Uri -like '*/git/ref/*') { $fixtureApi.RefResult } else { $fixtureApi.Result }
    if ($result -is [int]) {
        $response = [System.Net.Http.HttpResponseMessage]::new([Net.HttpStatusCode]$result)
        throw [Microsoft.PowerShell.Commands.HttpResponseException]::new('Fixture HTTP error', $response)
    }
    return $result
}
$priorGithubEnv = $env:GITHUB_ENV
$env:GITHUB_ENV = $null
try {
    foreach ($case in @(
        @{ Result = 404; Reject = $false },
        @{ Result = @{ draft = $true }; Reject = $false },
        @{ Result = @{ draft = $false }; Reject = $true },
        @{ Result = 500; Reject = $true }
    )) {
        $fixtureApi = [pscustomobject]@{ Result = $case.Result; RefResult = 404; Calls = 0 }
        $rejected = $false
        try { & (Join-Path $PSScriptRoot 'check-published-release-version.ps1') -RepositoryRoot $fixture -SourceBranch release/1.2 -RequestedType 'Branch default' } catch { $rejected = $true }
        if ($rejected -ne $case.Reject -or $fixtureApi.Calls -ne 1) { throw 'Published-version preflight made the wrong decision.' }
    }
    $fixtureApi.Calls = 0
    $rejected = $false
    try { & (Join-Path $PSScriptRoot 'check-published-release-version.ps1') -RepositoryRoot $fixture -SourceBranch release/1.3 } catch { $rejected = $true }
    if (-not $rejected -or $fixtureApi.Calls) { throw 'A mismatched release line was accepted.' }
    @{ version = '1.2.7'; channel = 'Canary'; releaseType = 'Canary' } | ConvertTo-Json | Set-Content -LiteralPath $statePath
    $fixtureApi = [pscustomobject]@{ Result = 404; RefResult = @{ ref = 'refs/heads/release/1.2' }; Calls = 0 }
    $rejected = $false
    try { & (Join-Path $PSScriptRoot 'check-published-release-version.ps1') -RepositoryRoot $fixture -SourceBranch main } catch { $rejected = $true }
    if (-not $rejected -or $fixtureApi.Calls -ne 1) { throw 'A Canary used a reserved release line.' }
    $fixtureApi.RefResult = 404
    $fixtureApi.Calls = 0
    & (Join-Path $PSScriptRoot 'check-published-release-version.ps1') -RepositoryRoot $fixture -SourceBranch main
    if ($fixtureApi.Calls -ne 2) { throw 'Canary reservation and publication checks were not both made.' }
} finally { $env:GITHUB_ENV = $priorGithubEnv }
Write-Host 'Release-line reservation, Canary patches, Beta-to-Stable promotion, duplicate publication, and version guards passed.'
