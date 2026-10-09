param([string]$MakensisPath)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
. (Join-Path $PSScriptRoot 'windows-maintenance-process.ps1')
if (-not $MakensisPath) { $MakensisPath = Join-Path $env:LOCALAPPDATA 'tauri/NSIS/makensis.exe' }
$fixtureRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('transmog-uninstall-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixtureRoot | Out-Null
$source = @'
Unicode true
RequestExecutionLevel user
SilentInstall silent
SilentUnInstall silent
Name "Transmog uninstall wait fixture"
OutFile "__INSTALLER__"
InstallDir "__INSTALLDIR__"
Section
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  FileOpen $0 "$INSTDIR\application.txt" w
  FileClose $0
SectionEnd
Section Uninstall
  Delete "$INSTDIR\application.txt"
  ; Reproduce cleanup that continues after the app executable disappears.
  Sleep 1500
  FileOpen $0 "__COMPLETION__" w
  FileWrite $0 "cleanup complete"
  FileClose $0
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  SetErrorLevel __EXITCODE__
SectionEnd
'@
foreach ($exitCode in @(0, 17)) {
    $installDirectory = Join-Path $fixtureRoot "install with spaces $exitCode"
    $installer = Join-Path $fixtureRoot "fixture-$exitCode.exe"
    $completion = Join-Path $fixtureRoot "completion-$exitCode.txt"
    $sourcePath = Join-Path $fixtureRoot "fixture-$exitCode.nsi"
    $source.Replace('__INSTALLER__', $installer).Replace('__INSTALLDIR__', $installDirectory).Replace('__COMPLETION__', $completion).Replace('__EXITCODE__', [string]$exitCode) |
        Set-Content -LiteralPath $sourcePath -Encoding utf8NoBOM
    & $MakensisPath /V2 $sourcePath
    Invoke-MaintenanceProcess -FilePath $installer -Arguments @('/S')
    $failure = $null
    try { Invoke-NsisUninstall -InstallDirectory $installDirectory -TemporaryDirectory $fixtureRoot } catch { $failure = $_ }
    if ($exitCode -eq 0 -and $failure) { throw $failure }
    if ($exitCode -ne 0 -and ($null -eq $failure -or $failure.ToString() -notmatch 'exit code 17')) { throw 'The uninstaller failure exit code was not preserved.' }
    if (-not (Test-Path -LiteralPath $completion)) { throw 'The uninstall wait ended before delayed cleanup completed.' }
    if (Test-Path -LiteralPath $installDirectory) { throw 'The fixture installation was not completely removed.' }
    if (@(Get-ChildItem -LiteralPath $fixtureRoot -Filter 'transmog-uninstall-*.exe').Count) { throw 'The temporary uninstaller copy was not removed.' }
}
Write-Host 'NSIS uninstall qualification waits for delayed cleanup and rejects a real failure exit code.'
