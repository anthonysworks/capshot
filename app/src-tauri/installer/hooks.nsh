; Extra steps of the installer made by Tauri (see tauri.conf.json).

!macro NSIS_HOOK_POSTUNINSTALL
  ; Capshot may have been set to start with Windows: remove that entry too.
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Capshot"
  ; "Delete the application data" ticked: also forget the settings and the
  ; history, so that a new install starts with the welcome setup again.
  ; Screenshots and recordings are never touched.
  ${If} $DeleteAppDataCheckboxState = 1
    RmDir /r "$APPDATA\Capshot"
    RmDir /r "$LOCALAPPDATA\Capshot"
  ${EndIf}
!macroend
