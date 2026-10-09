[CmdletBinding()]
param([string]$SevenZipPath)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repositoryRoot = Split-Path -Parent $PSScriptRoot
if (-not $SevenZipPath) {
    $command = Get-Command 7z,7za,7zz -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($command) { $SevenZipPath = $command.Source }
}
if (-not $SevenZipPath) {
    if (-not $IsWindows) { throw 'Install 7-Zip or pass SevenZipPath to run the independent ZIP reader test.' }
    $toolRoot = Join-Path $repositoryRoot '.tools/7zip-26.04'
    New-Item -ItemType Directory -Force -Path $toolRoot | Out-Null
    $assets = @(
        @{Name='7zr.exe';Sha256='256feca8e274e5da655e2a284fabafd9f554365eb164862089dacd4e8276d282'},
        @{Name='7z2604-extra.7z';Sha256='dc4b11d3399db18b063630137145f5585d8f7ac847bf3639bd1185d7d1f7cee0'}
    )
    foreach ($asset in $assets) {
        $path = Join-Path $toolRoot $asset.Name
        if (-not (Test-Path -LiteralPath $path)) {
            Invoke-WebRequest -Uri "https://github.com/ip7z/7zip/releases/download/26.04/$($asset.Name)" -OutFile $path
        }
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $asset.Sha256) { throw 'Official 7-Zip asset checksum mismatch.' }
    }
    & (Join-Path $toolRoot '7zr.exe') x (Join-Path $toolRoot '7z2604-extra.7z') "-o$toolRoot/portable" -y | Out-Host
    if ($LASTEXITCODE) { throw 'Portable native 7-Zip extraction failed.' }
    $SevenZipPath = Join-Path $toolRoot 'portable/x64/7za.exe'
}
$SevenZipPath = (Resolve-Path -LiteralPath $SevenZipPath).Path
$fixtureRoot = Join-Path $repositoryRoot "artifacts/saz-interop/$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Force -Path $fixtureRoot | Out-Null
$priorFixtureRoot = $env:TRANSMOG_SAZ_INTEROP_DIR
Push-Location $repositoryRoot
try {
    $env:TRANSMOG_SAZ_INTEROP_DIR = $fixtureRoot
    & cargo test --locked -p transmog-saz encrypted_saz_round_trips_and_produces_interop_fixtures
    if ($LASTEXITCODE) { throw 'Encrypted SAZ fixture generation failed.' }
    & cargo test --locked -p transmog-saz classic_metadata_required_dates_preserve_unavailable_and_original_times
    if ($LASTEXITCODE) { throw 'SAZ timer fixture generation failed.' }
    & (Join-Path $PSScriptRoot 'test-saz-metadata.ps1') -FixtureDirectory $fixtureRoot
    foreach ($profile in @('strict','extended')) {
        $encrypted = Join-Path $fixtureRoot "$profile-encrypted.saz"
        & $SevenZipPath t $encrypted '-pTransmog-interop-password' | Out-Host
        if ($LASTEXITCODE) { throw "7-Zip could not authenticate $profile SAZ." }
        $decoded = Join-Path $fixtureRoot "$profile-decoded"
        $reference = Join-Path $fixtureRoot "$profile-reference"
        & $SevenZipPath x $encrypted "-o$decoded" '-pTransmog-interop-password' -y | Out-Host
        if ($LASTEXITCODE) { throw '7-Zip decryption failed.' }
        & $SevenZipPath x (Join-Path $fixtureRoot "$profile-plain.saz") "-o$reference" -y | Out-Host
        if ($LASTEXITCODE) { throw '7-Zip reference extraction failed.' }
        $expected = @(Get-ChildItem -LiteralPath $reference -Recurse -File)
        if (@(Get-ChildItem -LiteralPath $decoded -Recurse -File).Count -ne $expected.Count) { throw 'Decrypted ZIP member count changed.' }
        foreach ($file in $expected) {
            $relative = [IO.Path]::GetRelativePath($reference,$file.FullName)
            $actual = Join-Path $decoded $relative
            if ((Get-FileHash -LiteralPath $actual).Hash -ne (Get-FileHash -LiteralPath $file.FullName).Hash) { throw "Decrypted bytes changed: $relative" }
        }
        $PSNativeCommandUseErrorActionPreference = $false
        try { & $SevenZipPath t $encrypted '-pincorrect' *>&1 | Out-Null; $wrongPasswordExit = $LASTEXITCODE }
        finally { $PSNativeCommandUseErrorActionPreference = $true }
        if ($wrongPasswordExit -eq 0) { throw 'Independent ZIP reader accepted an incorrect password.' }
        Write-Host "$profile SAZ: independent native decryption and exact member hashes verified."
    }
    $rawRoot = Join-Path $fixtureRoot 'legacy-source/raw'
    New-Item -ItemType Directory -Force -Path $rawRoot | Out-Null
    $payload = [byte[]]::new(262144)
    for ($index=0; $index -lt $payload.Length; $index++) { $payload[$index] = $index % 256 }
    $head = [Text.Encoding]::ASCII.GetBytes("POST https://example.test/upload HTTP/1.1`r`nContent-Length: $($payload.Length)`r`n`r`n")
    [IO.File]::WriteAllBytes((Join-Path $rawRoot '1_c.txt'),[byte[]]($head+$payload))
    Push-Location (Split-Path -Parent $rawRoot)
    try { & $SevenZipPath a -tzip (Join-Path $fixtureRoot 'external-zipcrypto.saz') 'raw/1_c.txt' '-mem=ZipCrypto' '-pTransmog-interop-password' | Out-Host }
    finally { Pop-Location }
    if ($LASTEXITCODE) { throw 'Independent ZipCrypto fixture creation failed.' }
    & cargo test --locked -p transmog-saz external_zipcrypto_imports_exact_binary_without_extraction
    if ($LASTEXITCODE) { throw 'Transmog could not import the independent encrypted binary fixture.' }
} finally {
    $env:TRANSMOG_SAZ_INTEROP_DIR = $priorFixtureRoot
    Pop-Location
}
