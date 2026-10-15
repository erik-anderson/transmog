# Keep Cargo, Tauri, updater, and public asset architecture names together.
# Tauri NSIS filenames use arm64; Rust triples and updater keys use aarch64.
function Get-WindowsReleaseTarget {
    param([ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')][string]$Target = 'x86_64-pc-windows-msvc')
    $arm64 = $Target -ceq 'aarch64-pc-windows-msvc'
    [pscustomobject]@{
        Target = $Target
        Architecture = $(if ($arm64) { 'arm64' } else { 'x64' })
        UpdaterPlatform = $(if ($arm64) { 'windows-aarch64-nsis' } else { 'windows-x86_64-nsis' })
        BuildSubdirectory = $(if ($arm64) { "target/$Target/release" } else { 'target/release' })
        Cli = $(if ($arm64) { 'transmog-cli-arm64.exe' } else { 'transmog-cli.exe' })
        PeMachine = $(if ($arm64) { 0xAA64 } else { 0x8664 })
    }
}

function Assert-WindowsBinaryTarget {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Target)
    $platform = Get-WindowsReleaseTarget $Target
    $reader = [IO.BinaryReader]::new([IO.File]::OpenRead($Path))
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) { throw "Missing executable header: $Path" }
        $reader.BaseStream.Position = 0x3C
        $peOffset = $reader.ReadUInt32()
        if ($peOffset -gt $reader.BaseStream.Length - 6) { throw "Invalid PE offset: $Path" }
        $reader.BaseStream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne $platform.PeMachine) { throw "Executable architecture differs from $Target : $Path" }
    } finally { $reader.Dispose() }
}
