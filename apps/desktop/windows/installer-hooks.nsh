; Transmog update/uninstall safety hooks. These run the currently installed
; binary in non-UI maintenance mode before NSIS replaces or removes it.
!include "${__FILEDIR__}\saz-association.nsh"
!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"

  ${If} $UpdateMode = 1
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --prepare-update' $0
  ${Else}
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-cleanup' $0
  ${EndIf}

  ${If} $0 != 0
    MessageBox MB_ICONSTOP|MB_OK "Transmog could not safely restore its Windows proxy/certificate state. The installer will stop without removing application files."
    SetErrorLevel 2
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro TRANSMOG_SAZ_INSTALL
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    !insertmacro TRANSMOG_SAZ_REMOVE
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "SazRegistration"
  ${EndIf}
!macroend
