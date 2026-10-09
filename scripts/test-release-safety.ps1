param()
. (Join-Path $PSScriptRoot 'windows-release-common.ps1')
$fixture = Join-Path ([System.IO.Path]::GetTempPath()) ('transmog-release-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force (Join-Path $fixture 'target\release') | Out-Null
$files = @(foreach ($name in @('transmog.exe', 'transmog-script-host.exe', 'transmog-preview-worker.exe', 'transmog-cli.exe')) {
    $path = Join-Path $fixture "target\release\$name"
    'fixture' | Set-Content -LiteralPath $path
    [ordered]@{ Path = "target/release/$name"; Sha256 = (Get-FileHash -LiteralPath $path).Hash }
})
$symbolsDirectory = Join-Path $fixture 'evidence'
New-Item -ItemType Directory -Path $symbolsDirectory | Out-Null
$symbolsPath = Join-Path $symbolsDirectory 'Transmog_0.1.0_windows-x64-symbols.zip'
'symbols archive fixture' | Set-Content -LiteralPath $symbolsPath
$files += [ordered]@{ Path = 'evidence/Transmog_0.1.0_windows-x64-symbols.zip'; Sha256 = (Get-FileHash -LiteralPath $symbolsPath).Hash }
$baseline = [ordered]@{ Commit = 'test-commit'; RunId = '123'; SourceBranch = 'main'; Target = 'x86_64-pc-windows-msvc'; Version = '0.1.0'; Channel = 'Canary'; ReleaseType = 'Canary'; Files = @($files) } | ConvertTo-Json -Depth 5
$manifestPath = Join-Path $fixture 'build-manifest.json'
function Set-TestManifest { $baseline | Set-Content -LiteralPath $manifestPath }
function Require-Rejection([scriptblock]$Action, [string]$Reason) {
    $rejected = $false
    try { & $Action | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw "Safety check failed to reject $Reason" }
    Write-Host "Rejected $Reason."
}
Set-TestManifest
Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' | Out-Null
Push-Location (Split-Path -Parent $fixture)
try {
    Assert-ReleasePayload -PayloadRoot (Split-Path -Leaf $fixture) -Commit 'test-commit' -RunId '123' | Out-Null
} finally { Pop-Location }
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'different-commit' -RunId '123' } 'a different source commit'
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '124' } 'a different workflow run'
$missingCli = $baseline | ConvertFrom-Json
$missingCli.Files = @($missingCli.Files | Where-Object { $_.Path -cne 'target/release/transmog-cli.exe' })
$missingCli | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'a missing standalone CLI payload'
Set-TestManifest
'tampered symbols' | Set-Content -LiteralPath $symbolsPath
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'changed PDB archive bytes'
'symbols archive fixture' | Set-Content -LiteralPath $symbolsPath
$missingSymbols = $baseline | ConvertFrom-Json
$missingSymbols.Files = @($missingSymbols.Files | Where-Object { $_.Path -cne 'evidence/Transmog_0.1.0_windows-x64-symbols.zip' })
$missingSymbols | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'a missing PDB archive'
Set-TestManifest
'tampered' | Set-Content -LiteralPath (Join-Path $fixture 'target\release\transmog.exe')
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'changed binary bytes'
'fixture' | Set-Content -LiteralPath (Join-Path $fixture 'target\release\transmog.exe')
$invalid = $baseline | ConvertFrom-Json
$invalid.Files[0].Path = '../outside.exe'
$invalid | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'directory traversal'
$invalid = $baseline | ConvertFrom-Json
$invalid.Files += $invalid.Files[0]
$invalid | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'duplicate catalog entries'
Set-TestManifest
'unlisted' | Set-Content -LiteralPath (Join-Path $fixture 'unexpected.ps1')
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'an unlisted payload file'
Write-Host 'Release payload safety checks passed.'
function Invoke-EnvironmentIsolatedFixture([string]$Script) {
    $before = [Environment]::GetEnvironmentVariables()
    & (Join-Path $PSScriptRoot $Script)
    $after = [Environment]::GetEnvironmentVariables()
    $names = @(@($before.Keys) + @($after.Keys) | Select-Object -Unique)
    foreach ($name in $names) {
        if (-not $before.Contains($name) -or -not $after.Contains($name) -or $before[$name] -cne $after[$name]) {
            throw "$Script changed the caller's environment variable $name."
        }
    }
    Write-Host "$Script preserved the caller's environment, including absent variables."
}
Invoke-EnvironmentIsolatedFixture 'test-release-publication.ps1'
Invoke-EnvironmentIsolatedFixture 'test-stable-update-release.ps1'
& (Join-Path $PSScriptRoot 'test-release-native-exit.ps1')
& (Join-Path $PSScriptRoot 'test-release-version.ps1')
Invoke-EnvironmentIsolatedFixture 'test-release-lifecycle.ps1'
