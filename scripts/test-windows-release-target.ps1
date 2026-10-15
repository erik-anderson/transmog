param()
. (Join-Path $PSScriptRoot 'windows-release-target.ps1')
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('transmog-target-' + [Guid]::NewGuid().ToString('N') + '.exe')
try {
    foreach ($target in @('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')) {
        $platform = Get-WindowsReleaseTarget $target
        $bytes = [byte[]]::new(128)
        [BitConverter]::GetBytes([uint16]0x5A4D).CopyTo($bytes, 0)
        [BitConverter]::GetBytes([uint32]64).CopyTo($bytes, 0x3C)
        [BitConverter]::GetBytes([uint32]0x4550).CopyTo($bytes, 64)
        [BitConverter]::GetBytes([uint16]$platform.PeMachine).CopyTo($bytes, 68)
        [IO.File]::WriteAllBytes($fixture, $bytes)
        Assert-WindowsBinaryTarget -Path $fixture -Target $target
        $other = if ($platform.Architecture -ceq 'arm64') { 'x86_64-pc-windows-msvc' } else { 'aarch64-pc-windows-msvc' }
        $rejected = $false
        try { Assert-WindowsBinaryTarget -Path $fixture -Target $other } catch { $rejected = $true }
        if (-not $rejected) { throw 'Accepted an executable for a different architecture.' }
    }
    Write-Host 'Release executable checks distinguish ARM64 from x64.'
} finally { Remove-Item -LiteralPath $fixture -ErrorAction SilentlyContinue }
