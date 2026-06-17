!macro customUnInstall
  ${If} $INSTDIR != ""
    ; 安装目录本身，程序会自动删，这里只处理 %APPDATA%\CCopy
    ReadRegStr $0 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\CCopy" "UninstallString"
    MessageBox MB_YESNO "是否删除剪贴板历史数据和缓存？" IDYES YES IDNO NO
YES:
    ; 删除 %APPDATA%\CCopy
    Delete "$APPDATA\CCopy\ccopy.db"
    Delete "$APPDATA\CCopy\blobs\*.*"
    RMDir "$APPDATA\CCopy\blobs\image"
    RMDir "$APPDATA\CCopy\blobs\other"
    RMDir "$APPDATA\CCopy\blobs"
    RMDir "$APPDATA\CCopy"
NO:
  ${EndIf}
!macroend