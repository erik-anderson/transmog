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
$evidenceTarget = $Target
$Stage = 'All'
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
    if ($record.Result -cne 'failed' -or $record.Commit -cne $sourceCommit -or $record.Target -cne $Target -or $record.Schema -ne 2) { throw 'Failure evidence lost its source or target binding.' }
    $rejected = $false
    try { Invoke-BuildPhase 'Assemble' { throw 'Must not assemble a failed test.' } } catch { $rejected = $_ -match 'requires a successful Tests' }
    if (-not $rejected) { throw 'Assembly accepted a failed test phase.' }
    Invoke-BuildPhase 'Tests' { }
    Invoke-BuildPhase 'Assemble' { }
    if ($state.Phases.Assemble.Result -cne 'passed') { throw 'Successful phases did not allow assembly.' }
    Invoke-BuildPhase 'Tests' { }
    if ($state.Phases.ContainsKey('Assemble')) { throw 'Rerunning an earlier gate retained stale downstream qualification.' }
    # A stage must skip unrelated gates and retain order within its own group.
    $phases = @('Analysis','Tests','Native','Archives')
    $Stage = 'Validation'
    $state = @{ Phases=@{} }
    Invoke-BuildPhase 'UI' { throw 'Validation must not repeat the frontend.' }
    $rejected = $false
    try { Invoke-BuildPhase 'Native' { throw 'Must not skip workspace tests.' } } catch { $rejected = $_ -match 'requires a successful Analysis' }
    if (-not $rejected) { throw 'An independent stage bypassed its own required gate.' }
    foreach ($name in $phases) { Invoke-BuildPhase $name { } }
    if ($state.Phases.Archives.Stage -cne 'Validation' -or $state.Phases.Native.Target -cne $Target) { throw 'Native stage ownership and target were not retained.' }
    $rejected = $false
    try { & (Join-Path $PSScriptRoot 'build-windows-release.ps1') -Stage Frontend -Target aarch64-pc-windows-msvc } catch { $rejected = $_ -match 'Shared frontend acceptance requires native Windows x64' }
    if (-not $rejected) { throw 'Frontend acceptance allowed an ARM64 execution target.' }
    $Stage = 'Frontend'
    $Target = 'x86_64-pc-windows-msvc'
    $identity = $ast.Find({ param($node) $node -is [Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -ceq '$evidenceTarget' }, $true)
    . ([scriptblock]::Create($identity.Extent.Text))
    $phases = @('UI','Browser')
    $state = @{ Phases=@{} }
    foreach ($name in $phases) { Invoke-BuildPhase $name { } }
    $record = Get-Content -LiteralPath (Join-Path $gateRoot 'Browser.json') -Raw | ConvertFrom-Json
    if ($record.Schema -ne 2 -or $record.Target -cne 'shared' -or $record.HostTarget -cne $Target -or $record.Stage -cne 'Frontend') { throw 'Shared frontend evidence lost its execution scope.' }
    Write-Host 'Release build phases preserve failures, source binding, execution scope and prerequisite order.'
} finally {
    $env:GITHUB_STEP_SUMMARY = $priorSummary
    $global:LASTEXITCODE = $priorExitCode
}
