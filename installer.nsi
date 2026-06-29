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
InstallDirRegKey HKLM "${REG_UNINST}" "InstallLocation"
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

; Check if Clippi is running, then ask user before terminating.
; Clippi is a tray app — closing its window only hides it, the process stays alive.
; So WM_CLOSE is not effective; we go straight to taskkill /F.
!macro CheckAndCloseApp un
Function ${un}CheckAndCloseApp
  FindWindow $0 "" "${APP_NAME}"
  IntCmp $0 0 done

  ${If} ${Silent}
    ; The in-app updater quits voluntarily after launching us. Give its normal
    ; shutdown path time to finish before using taskkill as a fallback.
    StrCpy $1 0
  wait_silent_exit:
    FindWindow $0 "" "${APP_NAME}"
    IntCmp $0 0 done
    Sleep 250
    IntOp $1 $1 + 1
    IntCmp $1 20 kill_silent wait_silent_exit kill_silent
  kill_silent:
    DetailPrint "Closing ${APP_NAME} after waiting for graceful shutdown..."
    nsExec::ExecToLog 'taskkill /F /IM ${APP_EXE}'
    Sleep 500
    Goto done
  ${EndIf}

  ; App is running — ask user
  MessageBox MB_YESNO|MB_ICONQUESTION \
    "${APP_NAME} 正在运行。$\r$\n$\r$\n点击「是」将关闭程序并继续，点击「否」取消。" \
    IDYES close IDNO cancel

cancel:
  Quit

close:
  DetailPrint "正在关闭 ${APP_NAME}..."
  nsExec::ExecToLog 'taskkill /F /IM ${APP_EXE}'
  Sleep 1000

done:
FunctionEnd
!macroend

; Installer .onInit
!insertmacro CheckAndCloseApp ""

Function .onInit
  Call CheckAndCloseApp
  Call RestoreExistingShortcutSections
FunctionEnd

; Uninstaller un.onInit
!insertmacro CheckAndCloseApp "un."

Function un.onInit
  Call un.CheckAndCloseApp
FunctionEnd

LangString DESC_Core ${LANG_SIMPCHINESE} "核心程序文件（必需）"
LangString DESC_StartMenu ${LANG_SIMPCHINESE} "在开始菜单创建快捷方式"
LangString DESC_Desktop ${LANG_SIMPCHINESE} "在桌面创建快捷方式"
LangString DESC_UnData ${LANG_SIMPCHINESE} "删除所有剪贴板历史记录、数据库、配置文件和日志"

Section "!Clippi" SectionCore
  SectionIn RO
  SetOutPath "$INSTDIR"

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

; Automatic updates use silent mode. Restart only after every selected
; section has completed successfully.
Function .onInstSuccess
  ${If} ${Silent}
    Exec '"$INSTDIR\${APP_EXE}"'
  ${EndIf}
FunctionEnd

Section "开始菜单快捷方式" SectionStartMenu
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\卸载 Clippi.lnk" "$INSTDIR\uninstall.exe"
SectionEnd

Section "桌面快捷方式" SectionDesktop
  CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
SectionEnd

Function RestoreExistingShortcutSections
  IfFileExists "$INSTDIR\${APP_EXE}" 0 done

  IfFileExists "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" 0 no_start_menu
  SectionSetFlags ${SectionStartMenu} 1
  Goto desktop

no_start_menu:
  SectionSetFlags ${SectionStartMenu} 0

desktop:
  IfFileExists "$DESKTOP\${APP_NAME}.lnk" 0 no_desktop
  SectionSetFlags ${SectionDesktop} 1
  Goto done

no_desktop:
  SectionSetFlags ${SectionDesktop} 0

done:
FunctionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SectionCore} $(DESC_Core)
  !insertmacro MUI_DESCRIPTION_TEXT ${SectionStartMenu} $(DESC_StartMenu)
  !insertmacro MUI_DESCRIPTION_TEXT ${SectionDesktop} $(DESC_Desktop)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section /o "un.清除应用数据" SectionUnData
  ; /o = unchecked by default — user must explicitly opt in to delete data
  ; Remove app data at %LOCALAPPDATA%\Clippi (database, config, images, logs)
  SetShellVarContext current
  RMDir /r "$LOCALAPPDATA\${APP_NAME}"
SectionEnd

!insertmacro MUI_UNFUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SectionUnData} $(DESC_UnData)
!insertmacro MUI_UNFUNCTION_DESCRIPTION_END

Section "Uninstall"
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\卸载 Clippi.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"
  Delete "$DESKTOP\${APP_NAME}.lnk"

  DeleteRegKey HKLM "${REG_UNINST}"
SectionEnd
