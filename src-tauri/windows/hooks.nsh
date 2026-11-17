; Installer hooks (Tauri NSIS).

; The installer's file details: Tauri's template leaves the company out.
VIAddVersionKey "CompanyName" "Software Hive S.R.L."
; Before uninstalling: remove the Explorer right-click menu the app registered (HKCU).
!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --unregister-shell'
!macroend
