; Optional current-user SAZ registration. Existing Windows user choices remain
; authoritative; never write Explorer's protected UserChoice key.
!ifndef TRANSMOG_CLASSES_KEY
  !define TRANSMOG_CLASSES_KEY "Software\Classes"
!endif
!ifndef TRANSMOG_REGISTERED_APPS_KEY
  !define TRANSMOG_REGISTERED_APPS_KEY "Software\RegisteredApplications"
!endif

Var SazRegistrationChoice
Var SazRegistrationCheckbox

!macro TRANSMOG_SAZ_INIT
  ClearErrors
  ReadRegDWORD $SazRegistrationChoice HKCU "${MANUPRODUCTKEY}" "SazRegistration"
  ${If} ${Errors}
    StrCpy $SazRegistrationChoice 0
  ${EndIf}
  ClearErrors
  ${GetOptions} $CMDLINE "/SAZ=" $0
  ${IfNot} ${Errors}
    ${If} $0 == "0"
      StrCpy $SazRegistrationChoice 0
    ${ElseIf} $0 == "1"
      StrCpy $SazRegistrationChoice 1
    ${Else}
      SetErrorLevel 2
      Abort
    ${EndIf}
  ${EndIf}
!macroend

!macro TRANSMOG_SAZ_PAGE
  Page custom TransmogSazPageCreate TransmogSazPageLeave
  Function TransmogSazPageCreate
    ${If} ${Silent}
    ${OrIf} $PassiveMode = 1
    ${OrIf} $UpdateMode = 1
      Abort
    ${EndIf}
    !insertmacro MUI_HEADER_TEXT "Open saved traffic" "Choose whether Transmog should be available for SAZ files."
    nsDialogs::Create 1018
    Pop $0
    ${If} $0 == error
      Abort
    ${EndIf}
    ${NSD_CreateLabel} 0 0 100% 48u "SAZ files contain saved HTTP traffic. Opening one in Transmog uses a capture viewer. If the main window is open, you can choose to import into its session or open a separate viewer."
    Pop $0
    ${NSD_CreateCheckbox} 0 56u 100% 20u "Register Transmog to open .saz files"
    Pop $SazRegistrationCheckbox
    ${NSD_SetState} $SazRegistrationCheckbox $SazRegistrationChoice
    ${NSD_CreateLabel} 0 84u 100% 40u "Transmog will appear in Open with and Windows default-app settings. Windows may ask you to choose it as the default. An existing default app is preserved."
    Pop $0
    nsDialogs::Show
  FunctionEnd
  Function TransmogSazPageLeave
    ${NSD_GetState} $SazRegistrationCheckbox $SazRegistrationChoice
  FunctionEnd
!macroend

!macro TRANSMOG_SAZ_REMOVE
  ReadRegStr $0 HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.Saz\shell\open\command" ""
  ${If} $0 == '$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\"'
    ReadRegStr $1 HKCU "${TRANSMOG_CLASSES_KEY}\.saz" ""
    ${If} $1 == "Transmog.Saz"
      DeleteRegValue HKCU "${TRANSMOG_CLASSES_KEY}\.saz" ""
    ${EndIf}
    DeleteRegValue HKCU "${TRANSMOG_CLASSES_KEY}\.saz\OpenWithProgids" "Transmog.Saz"
    DeleteRegKey /ifempty HKCU "${TRANSMOG_CLASSES_KEY}\.saz\OpenWithProgids"
    DeleteRegKey /ifempty HKCU "${TRANSMOG_CLASSES_KEY}\.saz"
    DeleteRegKey HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.Saz"
    ReadRegStr $1 HKCU "${TRANSMOG_REGISTERED_APPS_KEY}" "Transmog"
    ${If} $1 == "${MANUPRODUCTKEY}\Capabilities"
      DeleteRegValue HKCU "${TRANSMOG_REGISTERED_APPS_KEY}" "Transmog"
    ${EndIf}
    DeleteRegKey HKCU "${MANUPRODUCTKEY}\Capabilities"
    System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
  ${EndIf}
!macroend

!macro TRANSMOG_SAZ_INSTALL
  ${If} $SazRegistrationChoice = 1
    WriteRegStr HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.Saz" "" "SAZ traffic capture"
    WriteRegStr HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.Saz\DefaultIcon" "" '$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0'
    WriteRegStr HKCU "${TRANSMOG_CLASSES_KEY}\Transmog.Saz\shell\open\command" "" '$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\"'
    WriteRegStr HKCU "${TRANSMOG_CLASSES_KEY}\.saz\OpenWithProgids" "Transmog.Saz" ""
    WriteRegStr HKCU "${MANUPRODUCTKEY}\Capabilities" "ApplicationName" "Transmog"
    WriteRegStr HKCU "${MANUPRODUCTKEY}\Capabilities" "ApplicationDescription" "View and inspect saved HTTP traffic captures."
    WriteRegStr HKCU "${MANUPRODUCTKEY}\Capabilities" "ApplicationIcon" '$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0'
    WriteRegStr HKCU "${MANUPRODUCTKEY}\Capabilities\FileAssociations" ".saz" "Transmog.Saz"
    WriteRegStr HKCU "${TRANSMOG_REGISTERED_APPS_KEY}" "Transmog" "${MANUPRODUCTKEY}\Capabilities"
    !ifdef TRANSMOG_ASSOC_TESTING
      ReadRegStr $0 HKCU "${TRANSMOG_CLASSES_KEY}\.saz" ""
    !else
      ReadRegStr $0 HKCR ".saz" ""
    !endif
    ${If} $0 == ""
      WriteRegStr HKCU "${TRANSMOG_CLASSES_KEY}\.saz" "" "Transmog.Saz"
    ${EndIf}
    System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
  ${Else}
    !insertmacro TRANSMOG_SAZ_REMOVE
  ${EndIf}
  WriteRegDWORD HKCU "${MANUPRODUCTKEY}" "SazRegistration" $SazRegistrationChoice
!macroend
