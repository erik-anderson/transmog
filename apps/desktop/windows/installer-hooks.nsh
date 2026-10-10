; Transmog update/uninstall safety hooks. These run the currently installed
; binary in non-UI maintenance mode before NSIS replaces or removes it.
!include "${__FILEDIR__}\capture-associations.nsh"

!macro NSIS_HOOK_PREINSTALL
  ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
    ClearErrors
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --prepare-update' $0
    ${If} ${Errors}
      StrCpy $0 2
    ${EndIf}
    ${If} $0 != 0
      ${IfNot} ${Silent}
        MessageBox MB_ICONSTOP|MB_OK "Transmog could not finish its proxy and capture cleanup. Close Transmog and retry the update. No application files have been replaced."
      ${EndIf}
      SetErrorLevel 2
      Abort
    ${EndIf}
  ${EndIf}
!macroend
!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"

  ClearErrors
  ${If} $UpdateMode = 1
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --prepare-update' $0
  ${Else}
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-cleanup' $0
  ${EndIf}

  ${If} ${Errors}
    StrCpy $0 2
  ${EndIf}
  ${If} $0 != 0
    MessageBox MB_ICONSTOP|MB_OK "Transmog could not safely restore its Windows proxy/certificate state. The installer will stop without removing application files."
    SetErrorLevel 2
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro TRANSMOG_ASSOC_INSTALL
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    !insertmacro TRANSMOG_ASSOC_REMOVE
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "TmcapRegistration"
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "SazRegistration"
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "HarRegistration"
  ${EndIf}
!macroend
