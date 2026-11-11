; Reuse Tauri's installer; all resource discovery/deletion stays in Wukong.
Var WukongCleanupReview
Var WukongCleanupHelper
Var WukongCleanupArgs
Var WukongCleanupResult
Var WukongCleanupReport
Var WukongCleanupRan
Var WukongDeleteSettings
LangString WukongCleanupFailed 1033 "Cleanup did not complete. The program was retained. Review the error and retry. Log:"
LangString WukongCleanupFailed 2052 "清理未完成，程序已保留。请检查错误后重试。日志："
LangString WukongCleanupCancelled 1033 "Uninstall cancelled."
LangString WukongCleanupCancelled 2052 "已取消卸载。"

!macro NSIS_HOOK_PREUNINSTALL
  ; Upgrades/recovery must never opt into destructive cleanup.
  StrCpy $WukongCleanupRan 0
  StrCpy $WukongDeleteSettings 0
  ${If} $UpdateMode != 1
  ${AndIf} $PassiveMode != 1
    !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
    InitPluginsDir
    StrCpy $WukongCleanupReview "$PLUGINSDIR\cleanup-review.json"
    StrCpy $WukongCleanupHelper "$PLUGINSDIR\Wukong-Cleanup.exe"
    GetTempFileName $WukongCleanupReport
    CopyFiles /SILENT "$INSTDIR\${MAINBINARYNAME}.exe" "$WukongCleanupHelper"
    IfErrors wukong_cleanup_failed
    StrCpy $WukongCleanupArgs ""
    IfSilent wukong_cleanup_silent wukong_cleanup_interactive
    wukong_cleanup_interactive:
      ${If} $DeleteAppDataCheckboxState = 1
        StrCpy $WukongCleanupArgs "--initial-data"
      ${EndIf}
      ExecWait '"$WukongCleanupHelper" cleanup choose --output "$WukongCleanupReview" $WukongCleanupArgs' $WukongCleanupResult
      ${If} $WukongCleanupResult < 32
      ${OrIf} $WukongCleanupResult > 39
        MessageBox MB_OK "$(WukongCleanupCancelled)"
        Abort
      ${EndIf}
      IntOp $WukongDeleteSettings $WukongCleanupResult & 1
      Goto wukong_cleanup_execute
    wukong_cleanup_silent:
      ${GetParameters} $R0
      ClearErrors
      ${GetOptions} $R0 "/WUKONG_DELETE_DATA" $R1
      ${IfNot} ${Errors}
        StrCpy $WukongCleanupArgs "$WukongCleanupArgs --data"
        StrCpy $WukongDeleteSettings 1
      ${EndIf}
      ClearErrors
      ${GetOptions} $R0 "/WUKONG_DELETE_TOOLS" $R1
      ${IfNot} ${Errors}
        StrCpy $WukongCleanupArgs "$WukongCleanupArgs --tools"
      ${EndIf}
      ClearErrors
      ${GetOptions} $R0 "/WUKONG_DELETE_CHALLENGES" $R1
      ${IfNot} ${Errors}
        StrCpy $WukongCleanupArgs "$WukongCleanupArgs --challenges"
      ${EndIf}
      nsExec::ExecToLog '"$WukongCleanupHelper" cleanup plan $WukongCleanupArgs --output "$WukongCleanupReview"'
      Pop $WukongCleanupResult
      ${If} $WukongCleanupResult != 0
        Goto wukong_cleanup_failed
      ${EndIf}
    wukong_cleanup_execute:
      ; The isolated review window is closed before deleting its normal profile.
      nsExec::ExecToLog '"$WukongCleanupHelper" cleanup execute --review "$WukongCleanupReview" --yes --defer-finalize --report "$WukongCleanupReport"'
      Pop $WukongCleanupResult
      ${If} $WukongCleanupResult != 0
        Goto wukong_cleanup_failed
      ${EndIf}
      StrCpy $WukongCleanupRan 1
      Goto wukong_cleanup_done
    wukong_cleanup_failed:
      MessageBox MB_OK|MB_ICONSTOP "$(WukongCleanupFailed) $WukongCleanupReport" /SD IDOK
      SetErrorLevel 1
      Abort
    wukong_cleanup_done:
      ; The shared inventory owns data deletion, including framework profiles.
      ; Prevent Tauri from recursively deleting unreviewed/retained content.
      StrCpy $DeleteAppDataCheckboxState 0
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $WukongCleanupRan = 1
    nsExec::ExecToLog '"$WukongCleanupHelper" cleanup finish --review "$WukongCleanupReview"'
    Pop $WukongCleanupResult
    ${If} $WukongCleanupResult != 0
      MessageBox MB_OK|MB_ICONSTOP "$(WukongCleanupFailed) $WukongCleanupReport" /SD IDOK
      SetErrorLevel 1
    ${Else}
      ${If} $WukongDeleteSettings = 1
        DeleteRegKey SHCTX "${MANUPRODUCTKEY}"
        DeleteRegKey /ifempty SHCTX "${MANUKEY}"
      ${EndIf}
      Delete "$WukongCleanupReport"
    ${EndIf}
  ${EndIf}
!macroend
