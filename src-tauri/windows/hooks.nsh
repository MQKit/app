; Installer hooks (Tauri NSIS).

; The installer's file details: Tauri's template leaves the company out.
VIAddVersionKey "CompanyName" "Software Hive S.R.L."
; After installing: the Explorer menu, as the settings ask (an upgrade runs the previous
; version's uninstaller first, which takes it off).
!macro NSIS_HOOK_POSTINSTALL
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --register-shell'
!macroend

; Before uninstalling: remove the Explorer right-click menu the app registered (HKCU).
!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --unregister-shell'
!macroend
