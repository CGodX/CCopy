; NSIS installer script for CCopy
; Usage: makensis packager/CCopy.nsi

;--------------------------------
; Include Modern UI
!include "MUI2.nsh"

;--------------------------------
; General information
Name "CCopy"
OutFile "release\CCopy-v0.1.0-windows-installer.exe"
InstallDir "$PROGRAMFILES\CCopy"
Icon "packager\icon.ico"
UninstallIcon "packager\icon.ico"

;--------------------------------
; Requests application privileges
RequestExecutionLevel admin

;--------------------------------
; Enable close on click to title bar
SetCompressor lzma

;--------------------------------
; Include file mapping
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTALLFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTALLFILES
!insertmacro MUI_UNPAGE_FINISH

;--------------------------------
; Language file
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

;--------------------------------
; Files
Section "CCopy"
  SetOutPath "$INSTDIR"
  File "..\target\release\ccopy.exe"
  WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

;--------------------------------
; Uninstall
Section "Uninstall"
  ReadRegStr $0 HKLM Software\Microsoft\Windows\CurrentVersion\Uninstall\CCopy DisplayIcon
  DeleteRegKey HKLM Software\Microsoft\Windows\CurrentVersion\Uninstall\CCopy
  Delete $INSTDIR\ccopy.exe
  Delete $INSTDIR\uninstall.exe
  RMDir "$INSTDIR"
SectionEnd

;--------------------------------
; Add to Add/Remove Programs
WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\CCopy" \
  "DisplayName" "CCopy"
WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\CCopy" \
  "UninstallString" "$INSTDIR\uninstall.exe"
WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\CCopy" \
  "DisplayIcon" "$INSTDIR\ccopy.exe"
WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\CCopy" \
  "NoModify" 1
WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\CCopy" \
  "NoRepair" 1