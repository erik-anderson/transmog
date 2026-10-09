function Invoke-MaintenanceProcess([string]$FilePath, [string[]]$Arguments) {
    $process = Start-Process -FilePath $FilePath -ArgumentList $Arguments -PassThru -WindowStyle Hidden
    if (-not $process.WaitForExit(180000)) { Stop-Process -Id $process.Id -Force; throw 'Installer or maintenance process timed out.' }
    if ($process.ExitCode -ne 0) { throw "Maintenance process failed with exit code $($process.ExitCode)." }
}

function Invoke-NsisUninstall([string]$InstallDirectory, [string]$TemporaryDirectory) {
    # NSIS normally forks a temporary copy and immediately exits. Run our own
    # copy with _?= last so WaitForExit observes cleanup and its real exit code.
    # The copy lets NSIS remove the original uninstall.exe as it normally would.
    $copy = Join-Path $TemporaryDirectory ('transmog-uninstall-' + [Guid]::NewGuid().ToString('N') + '.exe')
    try {
        Copy-Item -LiteralPath (Join-Path $InstallDirectory 'uninstall.exe') -Destination $copy
        Invoke-MaintenanceProcess -FilePath $copy -Arguments @('/S', "_?=$InstallDirectory")
    } finally {
        Remove-Item -LiteralPath $copy -Force -ErrorAction SilentlyContinue
    }
}
