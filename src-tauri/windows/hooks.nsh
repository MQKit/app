; Installer hooks (Tauri NSIS).
; Before uninstalling: remove the Explorer right-click menu the app registered (HKCU).
!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --unregister-shell'
!macroend
