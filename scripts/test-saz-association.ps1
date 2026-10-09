param([string]$MakensisPath)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
if (-not $MakensisPath) { $MakensisPath = Join-Path $env:LOCALAPPDATA 'tauri\NSIS\makensis.exe' }
if (-not (Test-Path -LiteralPath $MakensisPath -PathType Leaf)) { throw 'Compile a development NSIS bundle first, or pass MakensisPath.' }
& (Join-Path $PSScriptRoot 'test-release-uninstall.ps1') -MakensisPath $MakensisPath
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$id = [Guid]::NewGuid().ToString('N')
$fixtureRoot = Join-Path $repositoryRoot "artifacts\saz-association-$id"
$registryRoot = "Software\TransmogAssociationTests\$id"
$registryPath = "HKCU:\$registryRoot"
if (Test-Path -LiteralPath $registryPath) { throw 'Association test namespace already exists.' }
New-Item -ItemType Directory -Path $fixtureRoot -Force | Out-Null
$source = @'
Unicode true
RequestExecutionLevel user
!include MUI2.nsh
!include FileFunc.nsh
!include nsDialogs.nsh
!define TRANSMOG_ASSOC_TESTING
!define TRANSMOG_CLASSES_KEY "__ROOT__\Classes"
!define TRANSMOG_REGISTERED_APPS_KEY "__ROOT__\RegisteredApplications"
!define MANUPRODUCTKEY "__ROOT__\Product"
!define MAINBINARYNAME "transmog"
!include "__INCLUDE__"
Var PassiveMode
Var UpdateMode
Name "Transmog SAZ association verification"
OutFile "__OUTPUT__"
InstallDir "C:\Transmog association fixture"
!insertmacro MUI_PAGE_WELCOME
!insertmacro TRANSMOG_SAZ_PAGE
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
Function .onInit
  !insertmacro TRANSMOG_SAZ_INIT
FunctionEnd
Section
  WriteRegStr HKCU "${TRANSMOG_CLASSES_KEY}\.saz" "" "Other.Saz"
  StrCpy $SazRegistrationChoice 1
  !insertmacro TRANSMOG_SAZ_INSTALL
  ReadRegStr $0 HKCU "${TRANSMOG_CLASSES_KEY}\.saz" ""
  WriteINIStr "__REPORT__" "Checks" "ExistingDefault" "$0"
  ReadRegStr $0 HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.Saz\shell\open\command" ""
  WriteINIStr "__REPORT__" "Checks" "OpenCommand" "$0"
  ReadRegStr $0 HKCU "${TRANSMOG_REGISTERED_APPS_KEY}" "Transmog"
  WriteINIStr "__REPORT__" "Checks" "RegisteredApp" "$0"
  !insertmacro TRANSMOG_SAZ_REMOVE
  ReadRegStr $0 HKCU "${TRANSMOG_CLASSES_KEY}\.saz" ""
  WriteINIStr "__REPORT__" "Checks" "DefaultAfterRemoval" "$0"
  ClearErrors
  ReadRegStr $0 HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.Saz\shell\open\command" ""
  ${If} ${Errors}
    WriteINIStr "__REPORT__" "Checks" "OwnedRegistrationRemoved" "true"
  ${EndIf}
  DeleteRegValue HKCU "${TRANSMOG_CLASSES_KEY}\.saz" ""
  !insertmacro TRANSMOG_SAZ_INSTALL
  ReadRegStr $0 HKCU "${TRANSMOG_CLASSES_KEY}\.saz" ""
  WriteINIStr "__REPORT__" "Checks" "UnassignedDefault" "$0"
  WriteRegStr HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.Saz\shell\open\command" "" "another installation"
  !insertmacro TRANSMOG_SAZ_REMOVE
  ReadRegStr $0 HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.Saz\shell\open\command" ""
  WriteINIStr "__REPORT__" "Checks" "AnotherInstallationPreserved" "$0"
SectionEnd
'@
$source = $source.Replace('__ROOT__', $registryRoot).Replace('__INCLUDE__', (Join-Path $repositoryRoot 'apps\desktop\windows\saz-association.nsh')).Replace('__OUTPUT__', (Join-Path $fixtureRoot 'association-fixture.exe')).Replace('__REPORT__', (Join-Path $fixtureRoot 'report.ini'))
$sourcePath = Join-Path $fixtureRoot 'fixture.nsi'
$source | Set-Content -LiteralPath $sourcePath -Encoding utf8NoBOM
try {
    & $MakensisPath /V2 $sourcePath
    $process = Start-Process -FilePath (Join-Path $fixtureRoot 'association-fixture.exe') -ArgumentList '/S' -PassThru -WindowStyle Hidden
    if (-not $process.WaitForExit(30000)) { Stop-Process -Id $process.Id -Force; throw 'Association fixture timed out.' }
    if ($process.ExitCode -ne 0) { throw 'Association fixture failed.' }
    $report = Get-Content -Raw -LiteralPath (Join-Path $fixtureRoot 'report.ini')
    foreach ($expected in @('ExistingDefault=Other.Saz', 'DefaultAfterRemoval=Other.Saz', 'OwnedRegistrationRemoved=true', 'UnassignedDefault=Transmog.Saz', 'AnotherInstallationPreserved=another installation')) {
        if (-not $report.Contains($expected)) { throw "Association behavior failed: $expected" }
    }
    if (-not $report.Contains('transmog.exe') -or -not $report.Contains('RegisteredApp=' + $registryRoot + '\Product\Capabilities')) { throw 'SAZ command or default-app registration is missing.' }
    Write-Host 'SAZ association checks passed in an isolated registry namespace.'
} finally {
    # This exact generated key is wholly owned by the fixture. Real Classes,
    # RegisteredApplications and Explorer UserChoice are never mutated.
    if ($registryRoot -notmatch '^Software\\TransmogAssociationTests\\[0-9a-f]{32}$') { throw 'Unsafe association test cleanup target.' }
    Remove-Item -LiteralPath $registryPath -Recurse -Force -ErrorAction SilentlyContinue
}
