param(
    [string]$ArchivePath
)

$ErrorActionPreference = 'Stop'
$expectedUrl = 'https://github.com/denoland/rusty_v8/releases/download/v150.4.0/rusty_v8_simdutf_release_x86_64-pc-windows-msvc.lib.gz'
$expectedSha256 = 'F231F82CBACB9AEFE6D9AF57E6DF2E8959A40E001F79306485133E3C075B98F0'
$escapedUrl = $expectedUrl -replace '[^A-Za-z0-9]', '_'
$cargoDirectory = if ($env:CARGO_HOME) {
    $env:CARGO_HOME
} else {
    Join-Path ([Environment]::GetFolderPath('UserProfile')) '.cargo'
}
$cacheDirectory = Join-Path $cargoDirectory '.rusty_v8'
$cachePath = Join-Path $cacheDirectory $escapedUrl
$temporaryDownload = $null

if (-not $ArchivePath) {
    $temporaryDownload = New-TemporaryFile
    Invoke-WebRequest -Uri $expectedUrl -OutFile $temporaryDownload.FullName
    $ArchivePath = $temporaryDownload.FullName
}

try {
    $resolvedArchive = (Resolve-Path -LiteralPath $ArchivePath).Path
    $actualSha256 = (Get-FileHash -LiteralPath $resolvedArchive -Algorithm SHA256).Hash
    if ($actualSha256 -ne $expectedSha256) {
        throw "rusty_v8 archive SHA-256 mismatch: $actualSha256"
    }
    New-Item -ItemType Directory -Force -Path $cacheDirectory | Out-Null
    Copy-Item -LiteralPath $resolvedArchive -Destination $cachePath -Force
    [pscustomobject]@{
        Version = '150.4.0'
        Sha256 = $actualSha256
        CachePath = $cachePath
    }
} finally {
    if ($null -ne $temporaryDownload) {
        Remove-Item -LiteralPath $temporaryDownload.FullName -Force -ErrorAction SilentlyContinue
    }
}
