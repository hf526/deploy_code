; 自定义 NSIS 钩子：卸载时若勾选"删除应用数据"，一并清理应用真实数据目录。
;
; deploy-core 的 Store 使用 directories::ProjectDirs::from("com", "deploycode", "DeployCode")，
; 在 Windows 上落在 %APPDATA%\deploycode\DeployCode\data（配置 / 部署历史 / 备份记录等）。
; Tauri 默认只删除 $APPDATA\<bundle identifier>（com.deploycode.app），与真实目录不一致，
; 因此在这里补充删除，保证卸载勾选"清除数据"时真正清干净。
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
    RMDir /r "$APPDATA\deploycode\DeployCode"
  ${EndIf}
!macroend
