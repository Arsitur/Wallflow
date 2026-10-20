; Only the opt-in entry owned by Wallflow is removed during uninstall.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode != 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Wallflow"
  ${EndIf}
!macroend
