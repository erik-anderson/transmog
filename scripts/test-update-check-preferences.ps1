param([string]$MakensisPath, [string]$ExecutablePath = 'target\debug\transmog.exe')
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repositoryRoot = Split-Path -Parent $PSScriptRoot
if (-not $MakensisPath) { $MakensisPath = Join-Path $env:LOCALAPPDATA 'tauri\NSIS\makensis.exe' }
if (-not (Test-Path -LiteralPath $MakensisPath -PathType Leaf)) { throw 'Compile a development NSIS bundle first, or pass MakensisPath.' }
$ExecutablePath = (Resolve-Path -LiteralPath $ExecutablePath).Path
$fixtureRoot = Join-Path $repositoryRoot ('target\ui-check\update-preferences-' + [Guid]::NewGuid().ToString('N'))
$profileRoot = Join-Path $fixtureRoot 'profile'
New-Item -ItemType Directory -Path $profileRoot -Force | Out-Null
$source = @'
Unicode true
RequestExecutionLevel user
!include MUI2.nsh
!include FileFunc.nsh
!include nsDialogs.nsh
!define MAINBINARYNAME "transmog"
!define MAINBINARYSRCPATH "__EXE__"
!include "__INCLUDE__"
Var PassiveMode
Var UpdateMode
Name "Transmog update preference verification"
OutFile "__OUTPUT__"
InstallDir "__INSTALL__"
!insertmacro MUI_PAGE_WELCOME
!insertmacro TRANSMOG_UPDATES_PAGE
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
Function .onInit
  !insertmacro TRANSMOG_UPDATES_INIT
  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}
FunctionEnd
Section
  SetOutPath "$INSTDIR"
  File "/oname=transmog.exe" "${MAINBINARYSRCPATH}"
  WriteINIStr "__REPORT__" "Checks" "Choice" "$AutomaticChecksChoice"
  WriteINIStr "__REPORT__" "Checks" "Touched" "$AutomaticChecksTouched"
  !insertmacro TRANSMOG_UPDATES_INSTALL
SectionEnd
'@
$source = $source.Replace('__EXE__', $ExecutablePath).Replace('__INCLUDE__', (Join-Path $repositoryRoot 'apps\desktop\windows\automatic-updates.nsh')).Replace('__OUTPUT__', (Join-Path $fixtureRoot 'fixture.exe')).Replace('__INSTALL__', (Join-Path $fixtureRoot 'installed')).Replace('__REPORT__', (Join-Path $fixtureRoot 'report.ini'))
$sourcePath = Join-Path $fixtureRoot 'fixture.nsi'
$source | Set-Content -LiteralPath $sourcePath -Encoding utf8NoBOM
& $MakensisPath /V2 $sourcePath
$previousLocal = $env:LOCALAPPDATA
function Invoke-PreferenceProcess([string]$File, [string[]]$Arguments, [int]$Expected = 0) {
    $process = Start-Process -FilePath $File -ArgumentList $Arguments -PassThru -WindowStyle Hidden
    if (-not $process.WaitForExit(30000)) { Stop-Process -Id $process.Id -Force; throw 'Update preference fixture timed out.' }
    if ($process.ExitCode -ne $Expected) { throw "Update preference command exited $($process.ExitCode), expected $Expected." }
}
function Assert-Choice([int]$Expected) {
    Invoke-PreferenceProcess $ExecutablePath @('--get-automatic-update-checks') $Expected
}
try {
    # Both installer and maintenance binaries inherit only this owned profile.
    $env:LOCALAPPDATA = $profileRoot
    $installer = Join-Path $fixtureRoot 'fixture.exe'
    Assert-Choice 0
    Invoke-PreferenceProcess $installer @('/S')
    $report = Get-Content -LiteralPath (Join-Path $fixtureRoot 'report.ini')
    if ('Choice=1' -cnotin $report -or 'Touched=0' -cnotin $report) { throw 'A fresh install did not default to enabled.' }
    Invoke-PreferenceProcess $installer @('/S', '/AUTOUPDATECHECKS=0')
    Assert-Choice 3
    $before = @(Get-ChildItem -LiteralPath (Join-Path $profileRoot 'Transmog') -Filter 'update-preferences.*.json').Name -join ','
    Invoke-PreferenceProcess $installer @('/S', '/UPDATE')
    Assert-Choice 3
    $after = @(Get-ChildItem -LiteralPath (Join-Path $profileRoot 'Transmog') -Filter 'update-preferences.*.json').Name -join ','
    if ($before -cne $after) { throw 'An untouched update rewrote the saved choice.' }
    Invoke-PreferenceProcess $installer @('/S', '/AUTOUPDATECHECKS=1')
    Assert-Choice 0
    Invoke-PreferenceProcess $installer @('/S', '/AUTOUPDATECHECKS=invalid') 2
    Assert-Choice 0
    Invoke-PreferenceProcess $ExecutablePath @('--set-automatic-update-checks=invalid') 2
    Invoke-PreferenceProcess $ExecutablePath @('--get-automatic-update-checks', '--unexpected') 2
    Assert-Choice 0
    $blockedProfile = Join-Path $fixtureRoot 'blocked-profile'
    New-Item -ItemType Directory -Path $blockedProfile | Out-Null
    Set-Content -LiteralPath (Join-Path $blockedProfile 'Transmog') -Value 'blocked preference directory'
    $env:LOCALAPPDATA = $blockedProfile
    Invoke-PreferenceProcess $installer @('/S', '/AUTOUPDATECHECKS=1') 2
    Assert-Choice 3
    Write-Host 'Installer default, opt-out, opt-in, update preservation, invalid arguments, and failed saves passed in an isolated profile.'
} finally {
    $env:LOCALAPPDATA = $previousLocal
}
