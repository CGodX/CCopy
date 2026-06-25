; NSIS installer script for CCopy
; Usage: makensis packager/CCopy.nsi

;--------------------------------
; Include Modern UI
!include "MUI2.nsh"
!include "FileFunc.nsh"

;--------------------------------
; General information
Name "CCopy"
OutFile "release\CCopy-v0.3.5-windows-installer.exe"
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
; 目录页离开时校验：末尾不是 CCopy 则自动追加 \CCopy
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE DirLeave
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
; 目录页离开回调：保证安装路径以 \CCopy 结尾
Function DirLeave
  ${GetFileName} "$INSTDIR" $0
  StrCmp "$0" "CCopy" +3 0
  StrCpy $INSTDIR "$INSTDIR\CCopy"
FunctionEnd

;--------------------------------
; Files
Section "CCopy"
  SetOutPath "$INSTDIR"
  File "..\target\release\ccopy.exe"
  ; 随程序附带图标资源，供快捷方式引用
  File "packager\icon.ico"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; 开始菜单快捷方式
  CreateDirectory "$SMPROGRAMS\CCopy"
  CreateShortCut "$SMPROGRAMS\CCopy\CCopy.lnk" "$INSTDIR\ccopy.exe" "" "$INSTDIR\icon.ico" 0
  CreateShortCut "$SMPROGRAMS\CCopy\卸载 CCopy.lnk" "$INSTDIR\uninstall.exe" "" "$INSTDIR\uninstall.exe" 0

  ; 桌面快捷方式
  CreateShortCut "$DESKTOP\CCopy.lnk" "$INSTDIR\ccopy.exe" "" "$INSTDIR\icon.ico" 0
SectionEnd

;--------------------------------
; Uninstall
Section "Uninstall"
  ; 删除快捷方式
  Delete "$SMPROGRAMS\CCopy\CCopy.lnk"
  Delete "$SMPROGRAMS\CCopy\卸载 CCopy.lnk"
  RMDir "$SMPROGRAMS\CCopy"
  Delete "$DESKTOP\CCopy.lnk"

  DeleteRegKey HKLM Software\Microsoft\Windows\CurrentVersion\Uninstall\CCopy
  Delete $INSTDIR\ccopy.exe
  Delete $INSTDIR\icon.ico
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
  "DisplayIcon" "$INSTDIR\icon.ico"
WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\CCopy" \
  "NoModify" 1
WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\CCopy" \
  "NoRepair" 1
