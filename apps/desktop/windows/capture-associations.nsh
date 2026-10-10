; Current-user capture handlers. Never modify Explorer's protected UserChoice.
!ifndef TRANSMOG_CLASSES_KEY
  !define TRANSMOG_CLASSES_KEY "Software\Classes"
!endif
!ifndef TRANSMOG_REGISTERED_APPS_KEY
  !define TRANSMOG_REGISTERED_APPS_KEY "Software\RegisteredApplications"
!endif

Var TmcapRegistrationChoice
Var SazRegistrationChoice
Var HarRegistrationChoice
Var TmcapRegistrationCheckbox
Var SazRegistrationCheckbox
Var HarRegistrationCheckbox

!macro TRANSMOG_ASSOC_CHOICE TYPE DEFAULT OPTION
  ClearErrors
  ReadRegDWORD $${TYPE}RegistrationChoice HKCU "${MANUPRODUCTKEY}" "${TYPE}Registration"
  ${If} ${Errors}
    StrCpy $${TYPE}RegistrationChoice ${DEFAULT}
  ${EndIf}
  ClearErrors
  ${GetOptions} $CMDLINE "/${OPTION}=" $0
  ${IfNot} ${Errors}
    ${If} $0 == "0"
      StrCpy $${TYPE}RegistrationChoice 0
    ${ElseIf} $0 == "1"
      StrCpy $${TYPE}RegistrationChoice 1
    ${Else}
      SetErrorLevel 2
      Abort
    ${EndIf}
  ${EndIf}
!macroend

!macro TRANSMOG_ASSOC_INIT
  !insertmacro TRANSMOG_ASSOC_CHOICE Tmcap 1 TMCAP
  !insertmacro TRANSMOG_ASSOC_CHOICE Saz 0 SAZ
  !insertmacro TRANSMOG_ASSOC_CHOICE Har 0 HAR
!macroend

!macro TRANSMOG_ASSOC_PAGE
  Page custom TransmogCapturePageCreate TransmogCapturePageLeave
  Function TransmogCapturePageCreate
    ${If} ${Silent}
    ${OrIf} $PassiveMode = 1
    ${OrIf} $UpdateMode = 1
      Abort
    ${EndIf}
    !insertmacro MUI_HEADER_TEXT "Open saved traffic" "Choose capture files to open with Transmog."
    nsDialogs::Create 1018
    Pop $0
    ${If} $0 == error
      Abort
    ${EndIf}
    ${NSD_CreateLabel} 0 0 100% 32u "Saved captures open in a capture viewer. If the main window is open, you can import there or open a separate viewer."
    Pop $0
    ${NSD_CreateCheckbox} 0 40u 100% 16u "Use Transmog for .tmcap files (native captures)"
    Pop $TmcapRegistrationCheckbox
    ${NSD_SetState} $TmcapRegistrationCheckbox $TmcapRegistrationChoice
    ${NSD_CreateCheckbox} 0 62u 100% 16u "Use Transmog for .saz files"
    Pop $SazRegistrationCheckbox
    ${NSD_SetState} $SazRegistrationCheckbox $SazRegistrationChoice
    ${NSD_CreateCheckbox} 0 84u 100% 16u "Use Transmog for .har files"
    Pop $HarRegistrationCheckbox
    ${NSD_SetState} $HarRegistrationCheckbox $HarRegistrationChoice
    ${NSD_CreateLabel} 0 112u 100% 42u "Transmog appears in Open with and Windows default-app settings. Files without a default use Transmog. Existing defaults are preserved; Windows may ask you to choose Transmog."
    Pop $0
    nsDialogs::Show
  FunctionEnd
  Function TransmogCapturePageLeave
    ${NSD_GetState} $TmcapRegistrationCheckbox $TmcapRegistrationChoice
    ${NSD_GetState} $SazRegistrationCheckbox $SazRegistrationChoice
    ${NSD_GetState} $HarRegistrationCheckbox $HarRegistrationChoice
  FunctionEnd
!macroend

!macro TRANSMOG_ASSOC_REMOVE_ONE EXT TYPE
  ReadRegStr $0 HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.${TYPE}\shell\open\command" ""
  ${If} $0 == '$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\"'
    ReadRegStr $1 HKCU "${TRANSMOG_CLASSES_KEY}\.${EXT}" ""
    ${If} $1 == "Transmog.${TYPE}"
      DeleteRegValue HKCU "${TRANSMOG_CLASSES_KEY}\.${EXT}" ""
    ${EndIf}
    DeleteRegValue HKCU "${TRANSMOG_CLASSES_KEY}\.${EXT}\OpenWithProgids" "Transmog.${TYPE}"
    DeleteRegKey /ifempty HKCU "${TRANSMOG_CLASSES_KEY}\.${EXT}\OpenWithProgids"
    DeleteRegKey /ifempty HKCU "${TRANSMOG_CLASSES_KEY}\.${EXT}"
    DeleteRegKey HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.${TYPE}"
    ReadRegStr $1 HKCU "${MANUPRODUCTKEY}\Capabilities\FileAssociations" ".${EXT}"
    ${If} $1 == "Transmog.${TYPE}"
      DeleteRegValue HKCU "${MANUPRODUCTKEY}\Capabilities\FileAssociations" ".${EXT}"
    ${EndIf}
  ${EndIf}
!macroend

!macro TRANSMOG_ASSOC_CLEANUP
  ReadRegStr $0 HKCU "${MANUPRODUCTKEY}\Capabilities\FileAssociations" ".tmcap"
  ReadRegStr $1 HKCU "${MANUPRODUCTKEY}\Capabilities\FileAssociations" ".saz"
  ReadRegStr $2 HKCU "${MANUPRODUCTKEY}\Capabilities\FileAssociations" ".har"
  ${If} "$0$1$2" == ""
    ReadRegStr $0 HKCU "${TRANSMOG_REGISTERED_APPS_KEY}" "Transmog"
    ${If} $0 == "${MANUPRODUCTKEY}\Capabilities"
      DeleteRegValue HKCU "${TRANSMOG_REGISTERED_APPS_KEY}" "Transmog"
      DeleteRegKey HKCU "${MANUPRODUCTKEY}\Capabilities"
    ${EndIf}
  ${EndIf}
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend

!macro TRANSMOG_ASSOC_REMOVE
  !insertmacro TRANSMOG_ASSOC_REMOVE_ONE tmcap Tmcap
  !insertmacro TRANSMOG_ASSOC_REMOVE_ONE saz Saz
  !insertmacro TRANSMOG_ASSOC_REMOVE_ONE har Har
  !insertmacro TRANSMOG_ASSOC_CLEANUP
!macroend

!macro TRANSMOG_ASSOC_INSTALL_ONE EXT TYPE DESCRIPTION
  ${If} $${TYPE}RegistrationChoice = 1
    WriteRegStr HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.${TYPE}" "" "${DESCRIPTION}"
    WriteRegStr HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.${TYPE}\DefaultIcon" "" '$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0'
    WriteRegStr HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.${TYPE}\shell\open\command" "" '$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\"'
    WriteRegStr HKCU "${TRANSMOG_CLASSES_KEY}\.${EXT}\OpenWithProgids" "Transmog.${TYPE}" ""
    WriteRegStr HKCU "${MANUPRODUCTKEY}\Capabilities" "ApplicationName" "Transmog"
    WriteRegStr HKCU "${MANUPRODUCTKEY}\Capabilities" "ApplicationDescription" "View and inspect saved HTTP traffic captures."
    WriteRegStr HKCU "${MANUPRODUCTKEY}\Capabilities" "ApplicationIcon" '$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0'
    WriteRegStr HKCU "${MANUPRODUCTKEY}\Capabilities\FileAssociations" ".${EXT}" "Transmog.${TYPE}"
    WriteRegStr HKCU "${TRANSMOG_REGISTERED_APPS_KEY}" "Transmog" "${MANUPRODUCTKEY}\Capabilities"
    !ifdef TRANSMOG_ASSOC_TESTING
      ReadRegStr $0 HKCU "${TRANSMOG_CLASSES_KEY}\.${EXT}" ""
    !else
      ReadRegStr $0 HKCR ".${EXT}" ""
    !endif
    ${If} $0 == ""
      WriteRegStr HKCU "${TRANSMOG_CLASSES_KEY}\.${EXT}" "" "Transmog.${TYPE}"
    ${EndIf}
  ${Else}
    !insertmacro TRANSMOG_ASSOC_REMOVE_ONE ${EXT} ${TYPE}
  ${EndIf}
  WriteRegDWORD HKCU "${MANUPRODUCTKEY}" "${TYPE}Registration" $${TYPE}RegistrationChoice
!macroend

!macro TRANSMOG_ASSOC_INSTALL
  !insertmacro TRANSMOG_ASSOC_INSTALL_ONE tmcap Tmcap "TMCap traffic capture"
  !insertmacro TRANSMOG_ASSOC_INSTALL_ONE saz Saz "SAZ traffic capture"
  !insertmacro TRANSMOG_ASSOC_INSTALL_ONE har Har "HAR traffic capture"
  !insertmacro TRANSMOG_ASSOC_CLEANUP
!macroend
