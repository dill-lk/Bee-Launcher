Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"

!define PRODUCT_NAME "Bee Launcher"
!define PRODUCT_VERSION "0.1.31"
!define PRODUCT_PUBLISHER "Bee Launcher"
!define APP_EXE "../target/x86_64-pc-windows-msvc/release/blockline-launcher.exe"
!define APP_ICON "../icons/icon.ico"
!define APP_USER_MODEL_ID "com.blockline.launcher"
!define SHORTCUT_METADATA_SCRIPT "SetShortcutAppId.ps1"

Name "${PRODUCT_NAME} ${PRODUCT_VERSION}"
  OutFile "../../release-artifacts/Bee-Launcher-Setup-0.1.31-Windows-x64.exe"
InstallDir "$LOCALAPPDATA\Programs\Bee Launcher"
InstallDirRegKey HKCU "Software\BlocklineLauncher" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

!define MUI_ICON "${APP_ICON}"
!define MUI_UNICON "${APP_ICON}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\blockline-launcher.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch Bee Launcher"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Install" SEC_INSTALL
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  File "${APP_EXE}"
  File "${APP_ICON}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  Delete "$SMPROGRAMS\Blockline Launcher\Blockline Launcher.lnk"
  RMDir "$SMPROGRAMS\Blockline Launcher"
  Delete "$DESKTOP\Blockline Launcher.lnk"
  CreateDirectory "$SMPROGRAMS\Bee Launcher"
  CreateShortcut "$SMPROGRAMS\Bee Launcher\Bee Launcher.lnk" "$INSTDIR\blockline-launcher.exe" "" "$INSTDIR\icon.ico" 0
  CreateShortcut "$DESKTOP\Bee Launcher.lnk" "$INSTDIR\blockline-launcher.exe" "" "$INSTDIR\icon.ico" 0
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "${SHORTCUT_METADATA_SCRIPT}"
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\${SHORTCUT_METADATA_SCRIPT}" -ExecutablePath "$INSTDIR\blockline-launcher.exe" -IconPath "$INSTDIR\icon.ico" -AppUserModelId "${APP_USER_MODEL_ID}" -ShortcutPaths "$SMPROGRAMS\Bee Launcher\Bee Launcher.lnk|$DESKTOP\Bee Launcher.lnk"' $0
  ${If} $0 != 0
    MessageBox MB_ICONEXCLAMATION|MB_OK "Bee Launcher installed, but Windows could not update the shortcut identity automatically. To refresh an old pinned icon, unpin Bee Launcher and pin it again from the Start menu."
  ${EndIf}
  WriteRegStr HKCU "Software\BlocklineLauncher" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\BlocklineLauncher" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\BlocklineLauncher" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\BlocklineLauncher" "DisplayIcon" "$INSTDIR\icon.ico"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\BlocklineLauncher" "Publisher" "${PRODUCT_PUBLISHER}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\BlocklineLauncher" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\BlocklineLauncher" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\BlocklineLauncher" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\BlocklineLauncher" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  Delete "$SMPROGRAMS\Bee Launcher\Bee Launcher.lnk"
  RMDir "$SMPROGRAMS\Bee Launcher"
  Delete "$DESKTOP\Bee Launcher.lnk"
  Delete "$SMPROGRAMS\Blockline Launcher\Blockline Launcher.lnk"
  RMDir "$SMPROGRAMS\Blockline Launcher"
  Delete "$DESKTOP\Blockline Launcher.lnk"
  Delete "$INSTDIR\blockline-launcher.exe"
  Delete "$INSTDIR\icon.ico"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "Software\BlocklineLauncher"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\BlocklineLauncher"
SectionEnd
