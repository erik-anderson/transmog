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
$files += @(foreach ($relative in @(
    'gates/UI.json','gates/Browser.json','gates/Analysis.json','gates/Tests.json',
    'gates/Native.json','gates/Archives.json','gates/Build.json','gates/Qualify.json',
    'policy/policy.json','policy/workspace.toml','policy/workspace.jsonl','policy/fuzz.toml','policy/fuzz.jsonl'
)) {
    $path = Join-Path $symbolsDirectory $relative
    New-Item -ItemType Directory -Force (Split-Path -Parent $path) | Out-Null
    if ($relative.StartsWith('gates/')) {
        @{ Name=[IO.Path]::GetFileNameWithoutExtension($relative); Commit='test-commit'; RunId='123'; Target='x86_64-pc-windows-msvc'; Result='passed' } | ConvertTo-Json | Set-Content -LiteralPath $path
    } else { 'evidence fixture' | Set-Content -LiteralPath $path }
    [ordered]@{ Path = "evidence/$relative"; Sha256 = (Get-FileHash -LiteralPath $path).Hash }
})
$symbolsPath = Join-Path $symbolsDirectory 'Transmog_0.1.0_windows-x64-symbols.zip'
'symbols archive fixture' | Set-Content -LiteralPath $symbolsPath
$files += [ordered]@{ Path = 'evidence/Transmog_0.1.0_windows-x64-symbols.zip'; Sha256 = (Get-FileHash -LiteralPath $symbolsPath).Hash }
$baseline = [ordered]@{ Commit = 'test-commit'; RunId = '123'; SourceBranch = 'main'; Target = 'x86_64-pc-windows-msvc'; Version = '0.1.0'; Channel = 'Canary'; ReleaseType = 'Canary'; Files = @($files) } | ConvertTo-Json -Depth 5
$manifestPath = Join-Path $fixture 'build-manifest.json'
function Set-TestManifest { $baseline | Set-Content -LiteralPath $manifestPath }
function Require-Rejection([scriptblock]$Action, [string]$Reason, [string]$ExpectedMessage) {
    $rejected = $false
    try { & $Action | Out-Null } catch { if ($ExpectedMessage -and $_ -notmatch $ExpectedMessage) { throw }; $rejected = $true }
    if (-not $rejected) { throw "Safety check failed to reject $Reason" }
    Write-Host "Rejected $Reason."
}
Set-TestManifest
Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' | Out-Null
$policyPath = Join-Path $symbolsDirectory 'policy/policy.json'
'tampered policy' | Set-Content -LiteralPath $policyPath
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'changed nested policy evidence'
'evidence fixture' | Set-Content -LiteralPath $policyPath
foreach ($relative in @('gates/Unexpected.json','gates/UI.exe','policy/unexpected.json','policy/workspace.exe','policy/db/advisory.md')) {
    $path = Join-Path $symbolsDirectory $relative
    New-Item -ItemType Directory -Force (Split-Path -Parent $path) | Out-Null
    'unexpected evidence' | Set-Content -LiteralPath $path
    $invalid = $baseline | ConvertFrom-Json
    $invalid.Files += [pscustomobject]@{ Path="evidence/$relative"; Sha256=(Get-FileHash -LiteralPath $path).Hash }
    $invalid | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath
    Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } "an unexpected nested evidence path: $relative"
    Remove-Item -LiteralPath $path
}
Set-TestManifest
$arm64 = $baseline | ConvertFrom-Json
$arm64.Target = 'aarch64-pc-windows-msvc'
foreach ($entry in $arm64.Files | Where-Object { $_.Path -like 'evidence/gates/*' }) {
    $path = Join-Path $fixture $entry.Path
    $gate = Get-Content -Raw $path | ConvertFrom-Json
    $gate.Target = $arm64.Target
    $gate | ConvertTo-Json | Set-Content -LiteralPath $path
    $entry.Sha256 = (Get-FileHash -LiteralPath $path).Hash
}
$armSymbolsPath = Join-Path $symbolsDirectory 'Transmog_0.1.0_windows-arm64-symbols.zip'
Move-Item -LiteralPath $symbolsPath -Destination $armSymbolsPath
$arm64.Files[-1].Path = 'evidence/Transmog_0.1.0_windows-arm64-symbols.zip'
$arm64 | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath
Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' | Out-Null
$arm64.Target = 'x86_64-pc-windows-msvc'
$arm64 | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'symbols from a different architecture'
Move-Item -LiteralPath $armSymbolsPath -Destination $symbolsPath
foreach ($entry in ($baseline | ConvertFrom-Json).Files | Where-Object { $_.Path -like 'evidence/gates/*' }) {
    $path = Join-Path $fixture $entry.Path
    $gate = Get-Content -Raw $path | ConvertFrom-Json
    $gate.Target = 'x86_64-pc-windows-msvc'
    $gate | ConvertTo-Json | Set-Content -LiteralPath $path
}
Set-TestManifest
Push-Location (Split-Path -Parent $fixture)
try {
    Assert-ReleasePayload -PayloadRoot (Split-Path -Leaf $fixture) -Commit 'test-commit' -RunId '123' | Out-Null
} finally { Pop-Location }
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'different-commit' -RunId '123' } 'a different source commit'
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '124' } 'a different workflow run'
foreach ($type in @($null, '', 'Unknown')) {
    $invalid = $baseline | ConvertFrom-Json
    $invalid.ReleaseType = $type
    $invalid | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath
    Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'missing or invalid package release type' 'explicit validated ReleaseType'
}
Set-TestManifest
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
Remove-Item -LiteralPath (Join-Path $fixture 'unexpected.ps1')
Set-TestManifest
Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' | Out-Null
$gatePath = Join-Path $fixture 'evidence/gates/Tests.json'
$originalGate = Get-Content -Raw $gatePath
$gate = $originalGate | ConvertFrom-Json
$gate.Result = 'failed'
$gate | ConvertTo-Json | Set-Content -LiteralPath $gatePath
$invalid = $baseline | ConvertFrom-Json
($invalid.Files | Where-Object Path -CEQ 'evidence/gates/Tests.json').Sha256 = (Get-FileHash -LiteralPath $gatePath).Hash
$invalid | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'a failed gate with a matching file catalog' 'Unqualified release gate: Tests'
$originalGate | Set-Content -NoNewline -LiteralPath $gatePath
Set-TestManifest
Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' | Out-Null
Write-Host 'Release payload safety checks passed.'
# Verify the complete merged schema at the same boundary used by signing.
$qualified = $baseline | ConvertFrom-Json
$qualified | Add-Member -NotePropertyName PipelineSchema -NotePropertyValue 2
$groups = @{ Frontend=@('UI','Browser'); Validation=@('Analysis','Tests','Native','Archives'); Package=@('Build','Qualify') }
$originalGates = @{}
foreach ($stage in $groups.Keys) {
    foreach ($name in $groups[$stage]) {
        $relative = "evidence/gates/$name.json"
        $path = Join-Path $fixture $relative
        $originalGates[$path] = Get-Content -Raw $path
        $gate = Get-Content -Raw $path | ConvertFrom-Json
        $gate | Add-Member -NotePropertyName Schema -NotePropertyValue 2
        $gate | Add-Member -NotePropertyName Stage -NotePropertyValue $stage
        $gate | Add-Member -NotePropertyName RunAttempt -NotePropertyValue '1'
        if ($stage -ceq 'Frontend') {
            $gate.Target = 'shared'
            $gate | Add-Member -NotePropertyName HostTarget -NotePropertyValue 'x86_64-pc-windows-msvc'
        }
        $gate | ConvertTo-Json | Set-Content -LiteralPath $path
        ($qualified.Files | Where-Object Path -CEQ $relative).Sha256 = (Get-FileHash $path).Hash
    }
}
$v8 = @{ Commit='test-commit'; RunId='123'; Target='x86_64-pc-windows-msvc'; ArtifactId='100'; ManifestSha256=('a' * 64) }
$pipeline = @{ Schema=2; Commit='test-commit'; RunId='123'; Target='x86_64-pc-windows-msvc'; RunAttempt='2'; V8=$v8; Stages=@{
    Frontend=@{ ArtifactId='101'; ManifestSha256=('b' * 64); RunAttempt='1'; Target='shared'; HostTarget='x86_64-pc-windows-msvc' }
    Validation=@{ ArtifactId='102'; ManifestSha256=('c' * 64); RunAttempt='1'; Target='x86_64-pc-windows-msvc' }
    Package=@{ ArtifactId='103'; ManifestSha256=('d' * 64); RunAttempt='1'; Target='x86_64-pc-windows-msvc' }
} }
foreach ($name in @('pipeline-inputs','v8-engine-validation','v8-engine-package')) {
    $value = if ($name -eq 'pipeline-inputs') { $pipeline } else { $v8 }
    $relative = "evidence/$name.json"
    $path = Join-Path $fixture $relative
    $value | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $path
    $qualified.Files += [pscustomobject]@{ Path=$relative; Sha256=(Get-FileHash $path).Hash }
}
$qualified | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath
Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' | Out-Null
$sharedGatePath = Join-Path $fixture 'evidence/gates/Browser.json'
$sharedGate = Get-Content -Raw -LiteralPath $sharedGatePath
foreach ($change in @(@{Target='aarch64-pc-windows-msvc'}, @{HostTarget='aarch64-pc-windows-msvc'}, @{Schema=1})) {
    $bad = $sharedGate | ConvertFrom-Json
    foreach ($key in $change.Keys) { $bad.$key = $change[$key] }
    $bad | ConvertTo-Json | Set-Content -LiteralPath $sharedGatePath
    ($qualified.Files | Where-Object Path -CEQ 'evidence/gates/Browser.json').Sha256 = (Get-FileHash $sharedGatePath).Hash
    $qualified | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath
    Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'mislabelled shared frontend evidence' 'Unqualified release gate: Browser'
}
$sharedGate | Set-Content -NoNewline -LiteralPath $sharedGatePath
($qualified.Files | Where-Object Path -CEQ 'evidence/gates/Browser.json').Sha256 = (Get-FileHash $sharedGatePath).Hash
foreach ($schema in @(1,3,$null)) {
    $bad = $qualified | ConvertTo-Json -Depth 8 | ConvertFrom-Json
    $bad.PipelineSchema = $schema
    $bad | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath
    Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'a downgraded or unknown pipeline schema' 'Unsupported or incomplete pipeline producer schema'
}
$pipelinePath = Join-Path $fixture 'evidence/pipeline-inputs.json'
$goodPipeline = Get-Content -Raw -LiteralPath $pipelinePath
$badPipeline = $goodPipeline | ConvertFrom-Json
$badPipeline.Stages.Frontend.HostTarget = 'aarch64-pc-windows-msvc'
$badPipeline | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $pipelinePath
($qualified.Files | Where-Object Path -CEQ 'evidence/pipeline-inputs.json').Sha256 = (Get-FileHash $pipelinePath).Hash
$qualified | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'a frontend producer claiming ARM64 execution' 'Stage producer execution scope differs'
$goodPipeline | Set-Content -NoNewline -LiteralPath $pipelinePath
($qualified.Files | Where-Object Path -CEQ 'evidence/pipeline-inputs.json').Sha256 = (Get-FileHash $pipelinePath).Hash
$qualified | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath
Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' | Out-Null
$path = Join-Path $fixture 'evidence/gates/Tests.json'
$goodGate = Get-Content -Raw $path
$gate = $goodGate | ConvertFrom-Json
$gate.Stage = 'Package'
$gate | ConvertTo-Json | Set-Content $path
($qualified.Files | Where-Object Path -CEQ 'evidence/gates/Tests.json').Sha256 = (Get-FileHash $path).Hash
$qualified | ConvertTo-Json -Depth 8 | Set-Content $manifestPath
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'a gate with the wrong producer' 'Release gate producer differs'
$goodGate | Set-Content -NoNewline $path
($qualified.Files | Where-Object Path -CEQ 'evidence/gates/Tests.json').Sha256 = (Get-FileHash $path).Hash
$path = Join-Path $fixture 'evidence/v8-engine-package.json'
$other = $v8.Clone()
$other.ArtifactId = '99'
$other | ConvertTo-Json | Set-Content $path
($qualified.Files | Where-Object Path -CEQ 'evidence/v8-engine-package.json').Sha256 = (Get-FileHash $path).Hash
$qualified | ConvertTo-Json -Depth 8 | Set-Content $manifestPath
Require-Rejection { Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' } 'different validated and packaged engines' 'V8 producer provenance differs'
foreach ($entry in $originalGates.GetEnumerator()) { $entry.Value | Set-Content -NoNewline $entry.Key }
foreach ($name in @('pipeline-inputs','v8-engine-validation','v8-engine-package')) { Remove-Item -LiteralPath (Join-Path $fixture "evidence/$name.json") }
Set-TestManifest
Assert-ReleasePayload -PayloadRoot $fixture -Commit 'test-commit' -RunId '123' | Out-Null
Write-Host 'Merged pipeline payload provenance checks passed.'

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
Invoke-EnvironmentIsolatedFixture 'test-hosted-client-certificate-policy.ps1'
& (Join-Path $PSScriptRoot 'test-release-native-exit.ps1')
& (Join-Path $PSScriptRoot 'test-windows-release-target.ps1')
& (Join-Path $PSScriptRoot 'test-release-build-phases.ps1')
& (Join-Path $PSScriptRoot 'test-signing-retry.ps1')
& (Get-TransmogPython) -m unittest discover -s (Join-Path $PSScriptRoot 'tests') -v
if ($LASTEXITCODE) { throw 'Portable tooling and release lifecycle fixtures failed.' }
