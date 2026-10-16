; JVM Desktop NSIS 卸载清理钩子（经 tauri.conf.json bundle.windows.nsis.installerHooks 引入）。
; 卸载（非静默、非更新模式）时：
; 1. 删除 workspace 下除 versions（JDK 安装目录）/downloads（下载缓存）外的
;    全部运行期内容（bin/current/logs/default/index.json/setting.json 等）；
; 2. 弹窗询问是否连 versions 与 downloads 一起删除，选「否」则保留；
; 3. 清理本工具写入的用户环境变量（仅当值仍指向本安装目录的 workspace，
;    用户自配的值不动），写后广播 WM_SETTINGCHANGE。
; 静默卸载（卸载程序以 /S 运行）与应用内更新（/UPDATE，旧版静默卸载）跳过
; 全部数据清理：更新时数据必须保留，静默时也不能被 MessageBox 卡住。

!ifndef JVM_DESKTOP_HOOK_CONSTANTS
  !define JVM_DESKTOP_HOOK_CONSTANTS
  !define JD_HWND_BROADCAST 0xFFFF
  !define JD_WM_SETTINGCHANGE 0x001A
!endif

!macro NSIS_HOOK_POSTUNINSTALL
  IfSilent jd_done 0
  StrCmp $UpdateMode "1" jd_done 0

  ; ---- 1. workspace 清理（保留 versions / downloads）----
  ; current 是 junction：先单独 RMDir 只删链接本身，绝不递归删除目标 JDK
  RMDir "$INSTDIR\workspace\current"
  ClearErrors
  FindFirst $R5 $R6 "$INSTDIR\workspace\*.*"
jd_ws_loop:
  StrCmp $R6 "" jd_ws_done 0
  StrCmp $R6 "." jd_ws_next 0
  StrCmp $R6 ".." jd_ws_next 0
  StrCmp $R6 "versions" jd_ws_next 0
  StrCmp $R6 "downloads" jd_ws_next 0
  StrCmp $R6 "current" jd_ws_next 0
  IfFileExists "$INSTDIR\workspace\$R6\*.*" 0 jd_ws_file
    RMDir /r "$INSTDIR\workspace\$R6"
    Goto jd_ws_next
jd_ws_file:
  Delete "$INSTDIR\workspace\$R6"
jd_ws_next:
  ClearErrors
  FindNext $R5 $R6
  IfErrors jd_ws_done jd_ws_loop
jd_ws_done:
  FindClose $R5

  ; ---- 2. 弹窗询问是否删除 JDK 安装目录与下载缓存 ----
  MessageBox MB_YESNO|MB_ICONQUESTION \
    "是否同时删除 JDK 安装目录与下载缓存？$\n$\n  $INSTDIR\workspace\versions$\n  $INSTDIR\workspace\downloads$\n$\n选择「否」将保留这两个目录，之后可手动清理。" \
    IDYES jd_del_data IDNO jd_env
jd_del_data:
  RMDir /r "$INSTDIR\workspace\versions"
  RMDir /r "$INSTDIR\workspace\downloads"
  RMDir "$INSTDIR\workspace"
  RMDir "$INSTDIR"

  ; ---- 3. 清理本工具写入的用户环境变量 ----
  ; NSIS 的 StrCmp 默认大小写不敏感，适合路径比较
jd_env:
  ReadRegStr $R0 HKCU "Environment" "JVM_HOME"
  StrCmp $R0 "$INSTDIR\workspace" 0 jd_env_2
    DeleteRegValue HKCU "Environment" "JVM_HOME"
jd_env_2:
  ReadRegStr $R0 HKCU "Environment" "JAVA_HOME"
  StrCmp $R0 "$INSTDIR\workspace\current" 0 jd_env_3
    DeleteRegValue HKCU "Environment" "JAVA_HOME"
jd_env_3:
  ; PATH：逐段扫描重建，移除 "$INSTDIR\workspace\bin" 条目，其余原样保留；
  ; 不用 StrFunc 的 ${UnStrRep}，避免卸载段函数声明时机问题
  ReadRegStr $R0 HKCU "Environment" "Path"
  StrCpy $R1 "$INSTDIR\workspace\bin"
  StrCpy $R2 ""        ; 重建结果
  StrCpy $R3 "0"       ; 是否已写入过条目
jd_path_scan:
  StrCmp $R0 "" jd_path_write 0
  StrCpy $R4 ""        ; 当前段
jd_path_chop:
  StrCmp $R0 "" jd_path_emit 0
  StrCpy $R5 $R0 1
  StrCmp $R5 ";" jd_path_consume 0
  StrCpy $R4 "$R4$R5"
  StrCpy $R0 $R0 "" 1
  Goto jd_path_chop
jd_path_consume:
  StrCpy $R0 $R0 "" 1
jd_path_emit:
  StrCmp $R4 "" jd_path_scan 0
  StrCmp $R4 $R1 jd_path_scan 0
  StrCmp $R3 "1" jd_path_semi 0
    StrCpy $R2 "$R4"
    StrCpy $R3 "1"
    Goto jd_path_scan
jd_path_semi:
  StrCpy $R2 "$R2;$R4"
  Goto jd_path_scan
jd_path_write:
  StrCmp $R3 "1" 0 jd_path_del
    WriteRegExpandStr HKCU "Environment" "Path" $R2
    Goto jd_env_done
jd_path_del:
  DeleteRegValue HKCU "Environment" "Path"
jd_env_done:
  SendMessage ${JD_HWND_BROADCAST} ${JD_WM_SETTINGCHANGE} 0 "STR:Environment"
jd_done:
!macroend
