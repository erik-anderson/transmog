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

$metadata = cargo metadata --format-version 1 --locked | ConvertFrom-Json -AsHashtable
. (Join-Path $PSScriptRoot 'release-version-common.ps1')
$releaseVersion = (Get-ReleaseVersionState $repositoryRoot).Version
$packagesById = @{}
foreach ($package in $metadata.packages) {
    $packagesById[$package.id] = $package
}

function Get-OrdinalSortKey([string]$Value) {
    return -join @($Value.ToCharArray() | ForEach-Object {
        # Keep the established underscore-before-hyphen notice ordering.
        $codePoint = if ($_ -eq '_') { 44 } else { [int]$_ }
        '{0:X4}' -f $codePoint
    })
}

$thirdParty = @($metadata.packages |
    Where-Object { $null -ne $_.source } |
    Sort-Object @{ Expression = { Get-OrdinalSortKey $_.name } },
                @{ Expression = { Get-OrdinalSortKey $_.version } })
$npmLockPath = Join-Path $repositoryRoot 'apps\desktop\ui\package-lock.json'
$npmProduction = @()
if (Test-Path -LiteralPath $npmLockPath) {
    $npmLock = Get-Content -Raw -LiteralPath $npmLockPath | ConvertFrom-Json -AsHashtable
    $npmProduction = @($npmLock.packages.GetEnumerator() | ForEach-Object {
        $packagePath = [string]$_.Key
        $package = $_.Value
        if (-not $packagePath -or $package.dev -eq $true -or -not $package.version) {
            return
        }
        $marker = 'node_modules/'
        $markerIndex = $packagePath.LastIndexOf($marker, [StringComparison]::Ordinal)
        if ($markerIndex -lt 0) {
            throw "Unexpected npm lockfile package path: $packagePath"
        }
        [pscustomobject]@{
            name = $packagePath.Substring($markerIndex + $marker.Length)
            version = [string]$package.version
            license = [string]$package.license
            source = [string]$package.resolved
        }
    } | Sort-Object @{ Expression = { Get-OrdinalSortKey $_.name } },
                    @{ Expression = { Get-OrdinalSortKey $_.version } })
}
$noticeLines = @(
    '# Third-party notices',
    '',
    'Generated from the locked Cargo graph and desktop npm production graph by `scripts/generate-supply-chain-artifacts.ps1`.',
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
    '## Desktop npm production packages',
    '',
    '| Package | Version | License expression | Source |',
    '|---|---:|---|---|'
)
foreach ($package in $npmProduction) {
    $noticeLines += "| $($package.name) | $($package.version) | $($package.license) | $($package.source) |"
}
$noticeLines += @(
    '',
    '## Bundled native code',
    '',
    '`boring-sys` and `quiche` compile bundled BoringSSL/quiche source. Their upstream license files and notices remain part of the Cargo source distribution and must be retained in binary distribution materials.'
)
$noticeDirectory = Split-Path -Parent ([System.IO.Path]::GetFullPath($NoticePath))
New-Item -ItemType Directory -Force -Path $noticeDirectory | Out-Null
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText(
    [System.IO.Path]::GetFullPath($NoticePath),
    (($noticeLines -join "`n") + "`n"),
    $utf8NoBom)

$components = @($metadata.packages |
    Sort-Object @{ Expression = { Get-OrdinalSortKey $_.name } },
                @{ Expression = { Get-OrdinalSortKey $_.version } } |
    ForEach-Object {
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
$components += @($npmProduction | ForEach-Object {
    $purlName = if ($_.name.StartsWith('@')) {
        $segments = $_.name.Substring(1).Split('/', 2)
        "%40$([Uri]::EscapeDataString($segments[0]))/$([Uri]::EscapeDataString($segments[1]))"
    } else {
        [Uri]::EscapeDataString($_.name)
    }
    [pscustomobject][ordered]@{
        type = 'library'
        'bom-ref' = "npm:$($_.name)@$($_.version)"
        name = $_.name
        version = $_.version
        licenses = @(@{ license = @{ expression = $_.license } })
        purl = "pkg:npm/$purlName@$([Uri]::EscapeDataString($_.version))"
        externalReferences = @(@{ type = 'distribution'; url = $_.source })
    }
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
        tools = @(@{ vendor = 'Transmog'; name = 'generate-supply-chain-artifacts.ps1' })
        component = @{
            type = 'application'
            name = 'transmog'
            version = $releaseVersion
        }
    }
    components = $components
    dependencies = $dependencies
}
$sbomDirectory = Split-Path -Parent ([System.IO.Path]::GetFullPath($SbomPath))
New-Item -ItemType Directory -Force -Path $sbomDirectory | Out-Null
[System.IO.File]::WriteAllText(
    [System.IO.Path]::GetFullPath($SbomPath),
    (($sbom | ConvertTo-Json -Depth 20) + "`n"),
    $utf8NoBom)

Write-Output "NOTICE_PATH=$([System.IO.Path]::GetFullPath($NoticePath))"
Write-Output "SBOM_PATH=$([System.IO.Path]::GetFullPath($SbomPath))"
