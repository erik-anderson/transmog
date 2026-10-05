; Transmog update/uninstall safety hooks. These run the currently installed
; binary in non-UI maintenance mode before NSIS replaces or removes it.
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
