; Extra steps of the installer made by Tauri (see tauri.conf.json).

; Capshot may have been set to start with Windows: remove that entry too.
!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Capshot"
!macroend
