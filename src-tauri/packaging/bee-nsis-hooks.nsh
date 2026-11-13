!include "LogicLib.nsh"

!macro NSIS_HOOK_PREINSTALL
  ; The previous Tauri setup app installed under this HKCU value. If the user
  ; proceeds with the NSIS default, keep upgrading in place instead of creating
  ; a second Bee Launcher install. The older NSIS prototype used the fallback key.
  Push $R0
  ${If} $INSTDIR == "$LOCALAPPDATA\Bee Launcher"
    ReadRegStr $R0 HKCU "Software\blockline\Bee Launcher" ""
    ${If} $R0 == ""
      ReadRegStr $R0 HKCU "Software\BlocklineLauncher" "InstallDir"
    ${EndIf}
    ${If} $R0 != ""
      IfFileExists "$R0\blockline-launcher.exe" BEE_MIGRATE_EXISTING BEE_KEEP_DEFAULT
      BEE_MIGRATE_EXISTING:
        StrCpy $INSTDIR "$R0"
        SetOutPath "$INSTDIR"
      BEE_KEEP_DEFAULT:
    ${EndIf}
  ${EndIf}
  Pop $R0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Keep both the current setup app and the earlier NSIS prototype pointed at
  ; this location so a later update can still find it.
  WriteRegStr HKCU "Software\blockline\Bee Launcher" "" "$INSTDIR"
  WriteRegStr HKCU "Software\BlocklineLauncher" "InstallDir" "$INSTDIR"
!macroend
