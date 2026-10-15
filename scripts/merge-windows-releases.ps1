param([Parameter(Mandatory)][string]$X64Root, [Parameter(Mandatory)][string]$Arm64Root, [Parameter(Mandatory)][string]$OutputRoot)
. (Join-Path $PSScriptRoot 'windows-release-common.ps1')
$x64 = Assert-SignedRelease -ReleaseRoot $X64Root -Commit $env:GITHUB_SHA -RunId $env:GITHUB_RUN_ID
$arm64 = Assert-SignedRelease -ReleaseRoot $Arm64Root -Commit $env:GITHUB_SHA -RunId $env:GITHUB_RUN_ID
if ($x64.Target -cne 'x86_64-pc-windows-msvc' -or $arm64.Target -cne 'aarch64-pc-windows-msvc') { throw 'Both Windows release architectures are required.' }
foreach ($field in @('Version', 'Channel', 'ReleaseType', 'Publisher', 'SourceBranch')) {
    if ($x64.$field -cne $arm64.$field) { throw "Windows release metadata differs: $field" }
}
if (Test-Path -LiteralPath $OutputRoot) { throw 'The merged release directory must be fresh.' }
New-Item -ItemType Directory -Path $OutputRoot | Out-Null
foreach ($source in @(@{ Root = $X64Root; Manifest = $x64 }, @{ Root = $Arm64Root; Manifest = $arm64 })) {
    foreach ($entry in $source.Manifest.Files) {
        if ($entry.Name -ceq 'latest.json') { continue }
        $name = $entry.Name
        # Keep existing x64 metadata asset names; namespace ARM64 evidence.
        if ($source.Manifest.Target -ceq 'aarch64-pc-windows-msvc' -and $name -notin @($arm64.Installer, "$($arm64.Installer).sig", $arm64.Cli, $arm64.Symbols)) { $name = "arm64-$name" }
        Copy-Item -LiteralPath (Join-Path $source.Root $entry.Name) -Destination (Join-Path $OutputRoot $name)
    }
}
$update = Get-Content -Raw -LiteralPath (Join-Path $X64Root 'latest.json') | ConvertFrom-Json -AsHashtable
$armUpdate = Get-Content -Raw -LiteralPath (Join-Path $Arm64Root 'latest.json') | ConvertFrom-Json -AsHashtable
$update.platforms['windows-aarch64-nsis'] = $armUpdate.platforms['windows-aarch64-nsis']
$update | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $OutputRoot 'latest.json') -Encoding utf8NoBOM
$manifest = $x64 | Select-Object * -ExcludeProperty Target
$packages = @(foreach ($release in @($x64, $arm64)) { $release | Select-Object Target, Installer, Cli, Symbols })
$manifest | Add-Member -NotePropertyName Packages -NotePropertyValue $packages
$manifest.Files = @(Get-ChildItem -LiteralPath $OutputRoot -File | ForEach-Object {
    [ordered]@{ Name = $_.Name; Sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }
})
$manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $OutputRoot 'release-manifest.json') -Encoding utf8NoBOM
$manifest.Files | ForEach-Object { "$($_.Sha256.ToLowerInvariant())  $($_.Name)" } | Set-Content -LiteralPath (Join-Path $OutputRoot 'SHA256SUMS') -Encoding utf8NoBOM
Assert-SignedRelease -ReleaseRoot $OutputRoot -Commit $env:GITHUB_SHA -RunId $env:GITHUB_RUN_ID | Out-Null
