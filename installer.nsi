; Clippi NSIS Installer
; VERSION and STAGING must be passed via command line:
;   makensis /DVERSION=x.y.z /DSTAGING=path installer.nsi

Unicode true
ManifestDPIAware true

!define APP_NAME "Clippi"
!define APP_PUBLISHER "Rains"
!define APP_EXE "clippi.exe"
!define REG_UNINST "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"

!ifndef VERSION
  !error "VERSION must be defined"
!endif
!ifndef STAGING
  !error "STAGING must be defined"
!endif

Name "${APP_NAME} ${VERSION}"
OutFile "${STAGING}\Clippi_${VERSION}_x64-setup.exe"
InstallDir "$PROGRAMFILES64\${APP_NAME}"
RequestExecutionLevel admin
SetCompressor /SOLID lzma

!include "MUI2.nsh"

!define MUI_ABORTWARNING
!define MUI_ICON "${STAGING}\app.ico"
!define MUI_UNICON "${STAGING}\app.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "启动 ${APP_NAME}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${STAGING}\LICENSE.txt"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_COMPONENTS
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"

LangString DESC_Core ${LANG_SIMPCHINESE} "核心程序文件（必需）"
LangString DESC_StartMenu ${LANG_SIMPCHINESE} "在开始菜单创建快捷方式"
LangString DESC_Desktop ${LANG_SIMPCHINESE} "在桌面创建快捷方式"
LangString DESC_UnData ${LANG_SIMPCHINESE} "删除所有剪贴板历史记录、数据库、配置文件和日志"

Section "!Clippi" SectionCore
  SectionIn RO
  SetOutPath "$INSTDIR"

  DetailPrint "Stopping running instance..."
  nsExec::ExecToLog 'taskkill /F /IM ${APP_EXE}'

  File "${STAGING}\${APP_EXE}"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr HKLM "${REG_UNINST}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKLM "${REG_UNINST}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr HKLM "${REG_UNINST}" "QuietUninstallString" "$\"$INSTDIR\uninstall.exe$\" /S"
  WriteRegStr HKLM "${REG_UNINST}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${REG_UNINST}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr HKLM "${REG_UNINST}" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKLM "${REG_UNINST}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${REG_UNINST}" "URLInfoAbout" "https://github.com/Ruszero01/clippi"
  WriteRegDWORD HKLM "${REG_UNINST}" "NoModify" 1
  WriteRegDWORD HKLM "${REG_UNINST}" "NoRepair" 1
SectionEnd

Section "开始菜单快捷方式" SectionStartMenu
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\卸载 Clippi.lnk" "$INSTDIR\uninstall.exe"
SectionEnd

Section "桌面快捷方式" SectionDesktop
  CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SectionCore} $(DESC_Core)
  !insertmacro MUI_DESCRIPTION_TEXT ${SectionStartMenu} $(DESC_StartMenu)
  !insertmacro MUI_DESCRIPTION_TEXT ${SectionDesktop} $(DESC_Desktop)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section /o "un.清除应用数据" SectionUnData
  ; /o = unchecked by default — user must explicitly opt in to delete data
  ; Remove app data at %LOCALAPPDATA%\Clippi (database, config, images, logs)
  RMDir /r "$LOCALAPPDATA\${APP_NAME}"
SectionEnd

!insertmacro MUI_UNFUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SectionUnData} $(DESC_UnData)
!insertmacro MUI_UNFUNCTION_DESCRIPTION_END

Section "Uninstall"
  nsExec::ExecToLog 'taskkill /F /IM ${APP_EXE}'

  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\卸载 Clippi.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"
  Delete "$DESKTOP\${APP_NAME}.lnk"

  DeleteRegKey HKLM "${REG_UNINST}"
SectionEnd
