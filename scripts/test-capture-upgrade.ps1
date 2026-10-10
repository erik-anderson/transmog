param([string]$MakensisPath)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
if (-not $MakensisPath) { $MakensisPath = Join-Path $env:LOCALAPPDATA 'tauri/NSIS/makensis.exe' }
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$fixtureRoot = Join-Path $repositoryRoot ('artifacts/capture-upgrade-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixtureRoot | Out-Null
$registryRoot = 'Software\TransmogUpgradeTests\' + [Guid]::NewGuid().ToString('N')
$registryPath = 'HKCU:\' + $registryRoot
if (Test-Path -LiteralPath $registryPath) { throw 'Upgrade test namespace already exists.' }
$template = Get-Content -Raw -LiteralPath (Join-Path $repositoryRoot 'apps/desktop/windows/installer.nsi')
$detection = [regex]::Match($template, '(?s)Var ExistingInstallation\s+Var ExistingVersion.*?FunctionEnd').Value
# Omit the page declaration: these fixtures exercise silent and passive modes.
$detection = $detection.Replace('Page custom PageReinstall', '')
if (-not $detection.Contains('Function DetectExistingInstallation')) { throw 'Upgrade detection function missing.' }
$helper = @'
Unicode true
RequestExecutionLevel user
SilentInstall silent
!include FileFunc.nsh
Name "Transmog owned update cleanup"
OutFile "__OLD_EXE__"
Function .onInit
  ${GetParameters} $0
  FileOpen $1 "__CLEANUP__" w
  FileWrite $1 "$0"
  FileClose $1
FunctionEnd
Section
  SetErrorLevel __CODE__
SectionEnd
'@
$source = @'
Unicode true
RequestExecutionLevel user
!include MUI2.nsh
!include FileFunc.nsh
!include nsDialogs.nsh
!define MAINBINARYNAME "transmog"
!define PRODUCTNAME "Transmog upgrade fixture"
!define MANUPRODUCTKEY "__ROOT__\Product"
!define UNINSTKEY "__ROOT__\Uninstall"
!define TRANSMOG_CLASSES_KEY "__ROOT__\Classes"
!define TRANSMOG_REGISTERED_APPS_KEY "__ROOT__\RegisteredApplications"
!define VERSION "2.0.0"
!define ALLOWDOWNGRADES "false"
!addplugindir "__PLUGINS__"
!macro CheckIfAppIsRunning PATH NAME
  ; Only this fixture owns and invokes its tiny maintenance executable.
!macroend
!include "__HOOKS__"
Var PassiveMode
Var UpdateMode
__DETECTION__
Name "Transmog upgrade qualification"
OutFile "__OUTPUT__"
InstallDir "__INSTALLDIR__"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
Function .onInit
  StrCpy $PassiveMode 1
  SetShellVarContext current
  Call DetectExistingInstallation
FunctionEnd
Section
  !insertmacro NSIS_HOOK_PREINSTALL
  FileOpen $1 "$INSTDIR\installed.txt" w
  FileWrite $1 "installed after cleanup"
  FileClose $1
SectionEnd
'@
try {
    New-Item -Path $registryPath -Force | Out-Null
    foreach ($case in @(
        @{Name='newer';Installed='1.0.0';Cleanup=0;Expected=0},
        @{Name='same';Installed='2.0.0';Cleanup=0;Expected=0},
        @{Name='downgrade';Installed='3.0.0';Cleanup=0;Expected=2},
        @{Name='cleanup-failure';Installed='1.0.0';Cleanup=17;Expected=2}
    )) {
        $installDirectory = Join-Path $fixtureRoot $case.Name
        New-Item -ItemType Directory -Path $installDirectory | Out-Null
        $cleanup = Join-Path $installDirectory 'cleanup.txt'
        $oldExe = Join-Path $installDirectory 'transmog.exe'
        $helperSource = Join-Path $fixtureRoot ($case.Name + '-old.nsi')
        $helper.Replace('__OLD_EXE__', $oldExe).Replace('__CLEANUP__', $cleanup).Replace('__CODE__', [string]$case.Cleanup) |
            Set-Content -LiteralPath $helperSource -Encoding utf8NoBOM
        & $MakensisPath /V2 $helperSource
        $oldHash = (Get-FileHash -LiteralPath $oldExe).Hash
        'existing preferences' | Set-Content -LiteralPath (Join-Path $installDirectory 'preferences.json')
        'saved capture' | Set-Content -LiteralPath (Join-Path $installDirectory 'saved.tmcap')
        $versionKey = New-Item -Path ($registryPath + '\Uninstall') -Force
        New-ItemProperty -LiteralPath $versionKey.PSPath -Name DisplayVersion -Value $case.Installed -Force | Out-Null
        New-ItemProperty -LiteralPath $versionKey.PSPath -Name UninstallString -Value 'must not execute an uninstaller' -Force | Out-Null
        $installer = Join-Path $fixtureRoot ($case.Name + '.exe')
        $sourcePath = Join-Path $fixtureRoot ($case.Name + '.nsi')
        $source.Replace('__ROOT__', $registryRoot).Replace('__PLUGINS__', (Join-Path (Split-Path -Parent $MakensisPath) 'Plugins/x86-unicode/additional')).Replace('__HOOKS__', (Join-Path $repositoryRoot 'apps/desktop/windows/installer-hooks.nsh')).Replace('__DETECTION__', $detection).Replace('__OUTPUT__', $installer).Replace('__INSTALLDIR__', $installDirectory) |
            Set-Content -LiteralPath $sourcePath -Encoding utf8NoBOM
        & $MakensisPath /V2 $sourcePath
        # Silent failures return a code without an interactive error dialog.
        $process = Start-Process -FilePath $installer -ArgumentList '/S' -PassThru -WindowStyle Hidden
        if (-not $process.WaitForExit(15000)) { Stop-Process -Id $process.Id -Force; throw "Upgrade fixture timed out: $($case.Name)" }
        if ($process.ExitCode -ne $case.Expected) { throw "Upgrade fixture $($case.Name): expected $($case.Expected), got $($process.ExitCode)." }
        if ((Test-Path -LiteralPath (Join-Path $installDirectory 'installed.txt')) -ne ($case.Expected -eq 0)) { throw 'Failed qualification changed application files.' }
        if ((Get-FileHash -LiteralPath $oldExe).Hash -cne $oldHash) { throw 'Old binary was modified before qualification.' }
        if ((Get-Content -Raw -LiteralPath (Join-Path $installDirectory 'preferences.json')).Trim() -cne 'existing preferences' -or (Get-Content -Raw -LiteralPath (Join-Path $installDirectory 'saved.tmcap')).Trim() -cne 'saved capture') { throw 'Upgrade qualification removed user data.' }
        if ($case.Name -eq 'downgrade') { if (Test-Path -LiteralPath $cleanup) { throw 'Downgrade ran cleanup before rejecting the version.' } }
        elseif ((Get-Content -Raw -LiteralPath $cleanup) -cne '--prepare-update') { throw 'Upgrade did not invoke update-only maintenance.' }
    }
    Write-Host 'In-place upgrade, same-version install, downgrade rejection and failed-cleanup preservation passed.'
} finally {
    if ($registryRoot -notmatch '^Software\\TransmogUpgradeTests\\[0-9a-f]{32}$') { throw 'Unsafe upgrade test cleanup target.' }
    Remove-Item -LiteralPath $registryPath -Recurse -Force -ErrorAction SilentlyContinue
}
