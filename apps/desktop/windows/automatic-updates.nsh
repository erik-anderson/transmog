; One app-owned updater preference; query the new bundled binary before install.
Var AutomaticChecksChoice
Var AutomaticChecksInitialChoice
Var AutomaticChecksTouched
Var AutomaticChecksCheckbox

!macro TRANSMOG_UPDATES_INIT
  InitPluginsDir
  File "/oname=$PLUGINSDIR\transmog-update-settings.exe" "${MAINBINARYSRCPATH}"
  ClearErrors
  ExecWait '"$PLUGINSDIR\transmog-update-settings.exe" --get-automatic-update-checks' $0
  ${If} ${Errors}
    StrCpy $0 2
  ${EndIf}
  ${If} $0 == 0
    StrCpy $AutomaticChecksChoice 1
  ${ElseIf} $0 == 3
    StrCpy $AutomaticChecksChoice 0
  ${Else}
    ${IfNot} ${Silent}
      MessageBox MB_ICONSTOP|MB_OK "Transmog update preferences could not be read. No application files have been changed."
    ${EndIf}
    SetErrorLevel 2
    Abort
  ${EndIf}
  StrCpy $AutomaticChecksInitialChoice $AutomaticChecksChoice
  StrCpy $AutomaticChecksTouched 0
  ClearErrors
  ${GetOptions} $CMDLINE "/AUTOUPDATECHECKS=" $0
  ${IfNot} ${Errors}
    ${If} $0 != 0
    ${AndIf} $0 != 1
      SetErrorLevel 2
      Abort
    ${EndIf}
    StrCpy $AutomaticChecksChoice $0
    StrCpy $AutomaticChecksTouched 1
  ${EndIf}
!macroend

!macro TRANSMOG_UPDATES_PAGE
  Page custom TransmogUpdatesPageCreate TransmogUpdatesPageLeave
  Function TransmogUpdatesPageCreate
    ${If} ${Silent}
    ${OrIf} $PassiveMode = 1
    ${OrIf} $UpdateMode = 1
      Abort
    ${EndIf}
    !insertmacro MUI_HEADER_TEXT "Update checks" "Choose whether Transmog checks for updates automatically."
    nsDialogs::Create 1018
    Pop $0
    ${If} $0 == error
      Abort
    ${EndIf}
    ${NSD_CreateCheckbox} 0 8u 100% 20u "Automatically check for updates"
    Pop $AutomaticChecksCheckbox
    ${NSD_SetState} $AutomaticChecksCheckbox $AutomaticChecksChoice
    ${NSD_CreateLabel} 0 40u 100% 48u "Transmog checks for a stable release when it starts. Installation always needs your choice. You can change this setting or check manually in Settings > About."
    Pop $0
    nsDialogs::Show
  FunctionEnd
  Function TransmogUpdatesPageLeave
    ${NSD_GetState} $AutomaticChecksCheckbox $AutomaticChecksChoice
    ${If} $AutomaticChecksChoice != $AutomaticChecksInitialChoice
      StrCpy $AutomaticChecksTouched 1
    ${EndIf}
  FunctionEnd
!macroend

!macro TRANSMOG_UPDATES_INSTALL
  ; Unchanged, silent and /UPDATE choices preserve the latest preference.
  ${If} $AutomaticChecksTouched = 1
    ClearErrors
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --set-automatic-update-checks=$AutomaticChecksChoice' $0
    ${If} ${Errors}
      StrCpy $0 2
    ${EndIf}
    ${If} $0 != 0
      ${IfNot} ${Silent}
        MessageBox MB_ICONSTOP|MB_OK "Transmog was installed, but the update-check preference could not be saved. The app will not be launched automatically. Resolve the preference storage problem and retry."
      ${EndIf}
      SetErrorLevel 2
      Abort
    ${EndIf}
  ${EndIf}
!macroend
