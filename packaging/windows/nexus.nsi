; Nexus installer (NSIS). Built by the Release workflow:
;   makensis /DVERSION=2.0.0 /DBIN=..\..\target\release /DOUT=..\..\dist\Nexus-Setup-2.0.0.exe nexus.nsi
; Per-user install, no administrator rights. Installs over the Electron builds (1.x) in the same folder and removes
; what they left behind.
Unicode true
!include "MUI2.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef BIN
  !define BIN "..\..\target\release"
!endif
!ifndef OUT
  !define OUT "Nexus-Setup-${VERSION}.exe"
!endif
!define APP "Nexus"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nexus"
; The uninstall entry electron-builder wrote for the 1.x builds (a GUID derived from the app id com.nexus.terminal).
!define OLD_UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\9cd1675a-a92d-5e13-bc1c-5eacc3a558fc"

Name "${APP}"
OutFile "${OUT}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\Nexus"
InstallDirRegKey HKCU "Software\Nexus" "InstallDir"
SetCompressor /SOLID lzma
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APP}"
VIAddVersionKey "FileDescription" "${APP} installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "mirrorsedgepro-stack"

!define MUI_ICON "..\..\assets\icon.ico"
!define MUI_UNICON "..\..\assets\icon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\Nexus.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Nexus"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Nexus"
  ; A running Nexus keeps its files locked: ask it to close first (its sessions on remote machines keep running in tmux).
  nsExec::Exec 'taskkill /IM Nexus.exe'
  Sleep 1500
  nsExec::Exec 'taskkill /F /IM Nexus.exe'
  Sleep 500

  ; Leftovers of an Electron (1.x) install in this folder. Only removed when that is what the folder holds.
  IfFileExists "$INSTDIR\resources\app.asar" 0 no_electron
    RMDir /r "$INSTDIR\resources"
    RMDir /r "$INSTDIR\locales"
    Delete "$INSTDIR\*.dll"
    Delete "$INSTDIR\*.pak"
    Delete "$INSTDIR\*.bin"
    Delete "$INSTDIR\*.dat"
    Delete "$INSTDIR\vk_swiftshader_icd.json"
    Delete "$INSTDIR\LICENSE.electron.txt"
    Delete "$INSTDIR\LICENSES.chromium.html"
    Delete "$INSTDIR\Uninstall Nexus.exe"
    DeleteRegKey HKCU "${OLD_UNINST_KEY}"
  no_electron:

  SetOutPath "$INSTDIR"
  File "/oname=Nexus.exe" "${BIN}\nexus.exe"
  File "${BIN}\nexus-mcp.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateShortcut "$SMPROGRAMS\Nexus.lnk" "$INSTDIR\Nexus.exe"
  CreateShortcut "$DESKTOP\Nexus.lnk" "$INSTDIR\Nexus.exe"

  WriteRegStr HKCU "Software\Nexus" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "mirrorsedgepro-stack"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\Nexus.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" "$0"
SectionEnd

; Settings (%APPDATA%\Nexus) are kept: machines, workspaces and pinned host keys survive a reinstall.
Section "Uninstall"
  nsExec::Exec 'taskkill /F /IM Nexus.exe'
  Sleep 500
  Delete "$INSTDIR\Nexus.exe"
  Delete "$INSTDIR\nexus-mcp.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Nexus.lnk"
  Delete "$DESKTOP\Nexus.lnk"
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\Nexus"
SectionEnd
