$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Get-WindowsSymbolEvidence {
    param([Parameter(Mandatory)][string]$Binary, [Parameter(Mandatory)][string]$Pdb)
    $toolDirectory = Join-Path (Split-Path -Parent $PSScriptRoot) '.tools/llvm/bin'
    $readObject = Join-Path $toolDirectory 'llvm-readobj.exe'
    $pdbUtility = Join-Path $toolDirectory 'llvm-pdbutil.exe'
    if (-not (Test-Path -LiteralPath $readObject)) { $readObject = (Get-Command llvm-readobj -ErrorAction Stop).Source }
    if (-not (Test-Path -LiteralPath $pdbUtility)) { $pdbUtility = (Get-Command llvm-pdbutil -ErrorAction Stop).Source }
    $binaryInfo = & $readObject --coff-debug-directory $Binary | Out-String
    if ($LASTEXITCODE -ne 0) { throw "Cannot read debug records: $Binary" }
    $pdbInfo = & $pdbUtility dump -summary $Pdb | Out-String
    if ($LASTEXITCODE -ne 0) { throw "Cannot read PDB: $Pdb" }
    $binaryGuid = [regex]::Matches($binaryInfo, 'PDBGUID: \{([A-Fa-f0-9-]+)\}')
    $binaryAge = [regex]::Matches($binaryInfo, 'PDBAge: (\d+)')
    $pdbGuid = [regex]::Matches($pdbInfo, '(?m)^\s*GUID: \{([A-Fa-f0-9-]+)\}')
    $pdbAge = [regex]::Matches($pdbInfo, '(?m)^\s*Age: (\d+)')
    if ($binaryGuid.Count -ne 1 -or $binaryAge.Count -ne 1 -or $pdbGuid.Count -ne 1 -or $pdbAge.Count -ne 1 -or
        $binaryGuid[0].Groups[1].Value -ine $pdbGuid[0].Groups[1].Value -or
        $binaryAge[0].Groups[1].Value -cne $pdbAge[0].Groups[1].Value) { throw "PDB identifier/age does not match executable: $Binary" }
    if ($pdbInfo -notmatch 'Has Debug Info: true' -or $pdbInfo -notmatch 'Is stripped: false') { throw "Full debug information is missing: $Pdb" }
    [ordered]@{
        Binary = [IO.Path]::GetFileName($Binary)
        UnsignedBinarySha256 = (Get-FileHash -LiteralPath $Binary -Algorithm SHA256).Hash
        Pdb = [IO.Path]::GetFileName($Pdb)
        PdbSha256 = (Get-FileHash -LiteralPath $Pdb -Algorithm SHA256).Hash
        Guid = $pdbGuid[0].Groups[1].Value
        Age = [uint32]$pdbAge[0].Groups[1].Value
    }
}

function New-WindowsReleaseSymbolArchive {
    param([Parameter(Mandatory)][string]$BuildDirectory, [Parameter(Mandatory)][string]$OutputPath,
        [Parameter(Mandatory)][string]$Version, [Parameter(Mandatory)][string]$Commit, [Parameter(Mandatory)][string]$RunId)
    if (Test-Path -LiteralPath $OutputPath) { throw 'A symbols archive already exists; use a fresh release build.' }
    $records = @(foreach ($binary in @('transmog.exe', 'transmog-script-host.exe', 'transmog-preview-worker.exe', 'transmog-cli.exe')) {
        $pdb = [IO.Path]::GetFileNameWithoutExtension($binary).Replace('-', '_') + '.pdb'
        Get-WindowsSymbolEvidence -Binary (Join-Path $BuildDirectory $binary) -Pdb (Join-Path $BuildDirectory $pdb)
    })
    $manifest = [ordered]@{ Version = $Version; Commit = $Commit; RunId = $RunId; Target = 'x86_64-pc-windows-msvc'; Files = $records } | ConvertTo-Json -Depth 5
    $archive = [IO.Compression.ZipFile]::Open([IO.Path]::GetFullPath($OutputPath), [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($record in $records) {
            [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, (Join-Path $BuildDirectory $record.Pdb), $record.Pdb, [IO.Compression.CompressionLevel]::Optimal) | Out-Null
        }
        $entry = $archive.CreateEntry('symbols-manifest.json')
        $writer = [IO.StreamWriter]::new($entry.Open(), [Text.UTF8Encoding]::new($false))
        try { $writer.WriteLine($manifest) } finally { $writer.Dispose() }
    } finally { $archive.Dispose() }
    Write-Host "Retained matching full PDBs for all four released executables in $([IO.Path]::GetFileName($OutputPath))."
}
