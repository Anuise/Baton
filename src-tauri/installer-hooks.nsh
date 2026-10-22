; Baton 的 NSIS 安裝程式掛勾（票 34）；由 tauri.conf.json 的 bundle.windows.nsis.installerHooks 引用。
;
; 兩個掛勾都呼叫已安裝資料夾裡的 baton-runtime.exe 維護輔助指令：要求執行中的 Runtime 進入維護
; （與完全退出相同的停止流程），等 Runtime 與監護結束；沒有 Runtime 時直接把維護抑制寫進生命週期檔，
; 讓監護與登入啟動入口不再拉起 Runtime。輔助指令是冪等的，除了用法錯誤都回傳 0。
; 以 nsExec::Exec 隱藏執行，不會閃出主控台視窗；結束碼以 Pop $0 取出後不影響安裝流程。
;
; 資料目錄 %LOCALAPPDATA%\Baton 永遠不會被刪除：Tauri 的解除安裝程式只刪除它安裝的檔案，
; 再以不遞迴的 RMDir 移除 $INSTDIR（不是空的就保留）；「刪除應用程式資料」勾選框只會刪除
; %LOCALAPPDATA%\app.baton.desktop（WebView 資料），與 Baton 的資料目錄無關。

; 覆蓋安裝（升級）前：舊版的 Runtime 與監護仍在執行時檔案無法覆蓋，先停止它們。
; 由 App 套用的更新已在維護中，輔助指令會略過要求維護，只等程序結束。
!macro NSIS_HOOK_PREINSTALL
  ${If} ${FileExists} "$INSTDIR\baton-runtime.exe"
    DetailPrint "正在停止 Baton Runtime…"
    nsExec::Exec '"$INSTDIR\baton-runtime.exe" maintenance prepare-update'
    Pop $0
  ${EndIf}
!macroend

; 解除安裝前：停止 Runtime 與監護並移除登入啟動登記（HKCU Run 值 Baton）。
; 更新模式（/UPDATE）是升級流程的一部分，不停止也不移除登記。
; Tauri 的解除安裝程式在非更新模式下也會刪除 HKCU Run 值 Baton，兩邊重複刪除無妨。
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ${If} ${FileExists} "$INSTDIR\baton-runtime.exe"
      DetailPrint "正在停止 Baton Runtime…"
      nsExec::Exec '"$INSTDIR\baton-runtime.exe" maintenance prepare-uninstall'
      Pop $0
    ${EndIf}
  ${EndIf}
!macroend
