param()
$ErrorActionPreference = 'Stop'
$tokens = $null
$errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'build-windows-release.ps1'), [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw 'Release build script does not parse.' }
$definition = $ast.Find({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq 'Invoke-BuildPhase' }, $true)
. ([scriptblock]::Create($definition.Extent.Text))
$root = Join-Path (Split-Path -Parent $PSScriptRoot) ('artifacts/tooling-tests/build-phases-' + [Guid]::NewGuid().ToString('N'))
$gateRoot = Join-Path $root 'gates'
New-Item -ItemType Directory -Force $gateRoot | Out-Null
$statePath = Join-Path $root 'state.json'
$sourceCommit = 'fixture-commit'
$Target = 'fixture-target'
$phases = @('UI','Tests','Assemble')
$state = @{ Phases=@{} }
$Phase = 'All'
$priorSummary = $env:GITHUB_STEP_SUMMARY
$priorExitCode = $global:LASTEXITCODE
try {
    $env:GITHUB_STEP_SUMMARY = $null
    $rejected = $false
    try { Invoke-BuildPhase 'Tests' { throw 'Must not execute without UI.' } } catch { $rejected = $_ -match 'requires a successful UI' }
    if (-not $rejected) { throw 'A build phase ran without its prerequisite.' }
    Invoke-BuildPhase 'UI' { }
    $rejected = $false
    try { Invoke-BuildPhase 'Tests' { $global:LASTEXITCODE = 7 } } catch { $rejected = $_ -match 'exited with 7' }
    if (-not $rejected -or $state.Phases.Tests.Result -cne 'failed') { throw 'Native command failure was not recorded.' }
    $record = Get-Content -LiteralPath (Join-Path $gateRoot 'Tests.json') -Raw | ConvertFrom-Json
    if ($record.Result -cne 'failed' -or $record.Commit -cne $sourceCommit) { throw 'Failure evidence lost its source binding.' }
    $rejected = $false
    try { Invoke-BuildPhase 'Assemble' { throw 'Must not assemble a failed test.' } } catch { $rejected = $_ -match 'requires a successful Tests' }
    if (-not $rejected) { throw 'Assembly accepted a failed test phase.' }
    Invoke-BuildPhase 'Tests' { }
    Invoke-BuildPhase 'Assemble' { }
    if ($state.Phases.Assemble.Result -cne 'passed') { throw 'Successful phases did not allow assembly.' }
    Invoke-BuildPhase 'Tests' { }
    if ($state.Phases.ContainsKey('Assemble')) { throw 'Rerunning an earlier gate retained stale downstream qualification.' }
    Write-Host 'Release build phases preserve failures, source binding and prerequisite order.'
} finally {
    $env:GITHUB_STEP_SUMMARY = $priorSummary
    $global:LASTEXITCODE = $priorExitCode
}
