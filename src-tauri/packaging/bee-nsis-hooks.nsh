!include "LogicLib.nsh"
!define BEE_HOOK_DIR "${__FILEDIR__}"

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

  ; The legacy Bee installer always created a Desktop shortcut. Tauri's NSIS
  ; finish-page option is optional, so create it here as well for upgrades.
  SetShellVarContext current
  CreateShortcut "$DESKTOP\Bee Launcher.lnk" "$INSTDIR\blockline-launcher.exe" "" "$INSTDIR\blockline-launcher.exe" 0

  ; Restore the former icon/AppUserModelID repair for Start Menu, Desktop, and
  ; existing taskbar pins that target this installation.
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "${BEE_HOOK_DIR}/SetShortcutAppId.ps1"
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\SetShortcutAppId.ps1" -ExecutablePath "$INSTDIR\blockline-launcher.exe" -IconPath "$INSTDIR\blockline-launcher.exe" -AppUserModelId "com.blockline.launcher" -ShortcutPaths "$SMPROGRAMS\$AppStartMenuFolder\Bee Launcher.lnk|$DESKTOP\Bee Launcher.lnk"' $0
  ${If} $0 != 0
    MessageBox MB_ICONEXCLAMATION|MB_OK "Bee Launcher installed, but Windows could not update the shortcut identity automatically. To refresh an old pinned icon, unpin Bee Launcher and pin it again from the Start menu."
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Remove Desktop and pre-branding shortcuts; Tauri removes the current Start Menu link.
  SetShellVarContext current
  Delete "$DESKTOP\Bee Launcher.lnk"
  Delete "$SMPROGRAMS\Blockline Launcher\Blockline Launcher.lnk"
  RMDir "$SMPROGRAMS\Blockline Launcher"
  Delete "$DESKTOP\Blockline Launcher.lnk"
!macroend
