param(
    [string]$NoticePath,
    [string]$SbomPath
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repositoryRoot = Split-Path -Parent $PSScriptRoot
if (-not $NoticePath) {
    $NoticePath = Join-Path $repositoryRoot 'THIRD_PARTY_NOTICES.md'
}
if (-not $SbomPath) {
    $SbomPath = Join-Path $repositoryRoot 'artifacts\sbom.cdx.json'
}

$metadata = cargo metadata --format-version 1 --locked | ConvertFrom-Json
$packagesById = @{}
foreach ($package in $metadata.packages) {
    $packagesById[$package.id] = $package
}

$thirdParty = @($metadata.packages |
    Where-Object { $null -ne $_.source } |
    Sort-Object name, version)
$noticeLines = @(
    '# Third-party notices',
    '',
    'Generated from the locked Cargo graph by `scripts/generate-supply-chain-artifacts.ps1`.',
    'Distributions must retain the license texts shipped by their dependencies and bundled native sources.',
    '',
    '| Package | Version | License expression | Source |',
    '|---|---:|---|---|'
)
foreach ($package in $thirdParty) {
    $repository = if ($package.repository) { $package.repository } else { $package.source }
    $noticeLines += "| $($package.name) | $($package.version) | $($package.license) | $repository |"
}
$noticeLines += @(
    '',
    '## Bundled native code',
    '',
    '`boring-sys` and `quiche` compile bundled BoringSSL/quiche source. Their upstream license files and notices remain part of the Cargo source distribution and must be retained in binary distribution materials.'
)
$noticeDirectory = Split-Path -Parent ([System.IO.Path]::GetFullPath($NoticePath))
New-Item -ItemType Directory -Force -Path $noticeDirectory | Out-Null
Set-Content -LiteralPath $NoticePath -Value ($noticeLines -join "`n") -Encoding utf8

$components = @($metadata.packages | Sort-Object name, version | ForEach-Object {
    $component = [ordered]@{
        type = if ($null -eq $_.source) { 'application' } else { 'library' }
        'bom-ref' = $_.id
        name = $_.name
        version = $_.version
        licenses = @(@{ license = @{ expression = $_.license } })
        purl = "pkg:cargo/$([Uri]::EscapeDataString($_.name))@$([Uri]::EscapeDataString($_.version))"
    }
    if ($_.repository) {
        $component.externalReferences = @(@{ type = 'vcs'; url = $_.repository })
    }
    [pscustomobject]$component
})
$dependencies = @($metadata.resolve.nodes | ForEach-Object {
    [pscustomobject]@{
        ref = $_.id
        dependsOn = @($_.dependencies)
    }
})
$sbom = [ordered]@{
    bomFormat = 'CycloneDX'
    specVersion = '1.5'
    version = 1
    metadata = @{
        timestamp = [DateTimeOffset]::UtcNow.ToString('o')
        tools = @(@{ vendor = 'rustymiddle'; name = 'generate-supply-chain-artifacts.ps1' })
        component = @{
            type = 'application'
            name = 'rustymiddle'
            version = '0.1.0'
        }
    }
    components = $components
    dependencies = $dependencies
}
$sbomDirectory = Split-Path -Parent ([System.IO.Path]::GetFullPath($SbomPath))
New-Item -ItemType Directory -Force -Path $sbomDirectory | Out-Null
$sbom | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $SbomPath -Encoding utf8

Write-Output "NOTICE_PATH=$([System.IO.Path]::GetFullPath($NoticePath))"
Write-Output "SBOM_PATH=$([System.IO.Path]::GetFullPath($SbomPath))"
