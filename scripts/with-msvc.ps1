# 在含 C++ x64 工具元件的 Visual Studio 環境中執行指令。
# 用法：powershell -File scripts/with-msvc.ps1 cargo test
# 用途：rustc 預設可能挑到「未安裝 C++ 桌面工作負載」的較新 VS，導致 LNK1104 找不到 msvcrt.lib。
# 已有正確環境（例如開發人員命令提示字元）時不需要使用本腳本。
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$root = & $vswhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -latest -property installationPath
if (-not $root) {
    Write-Error '找不到含 C++ x64 工具元件的 Visual Studio，請安裝「使用 C++ 的桌面開發」工作負載。'
    exit 1
}
$vcvars = Join-Path $root 'VC\Auxiliary\Build\vcvars64.bat'
# 執行 vcvars64.bat 後把環境變數帶回目前程序
cmd /c "`"$vcvars`" >nul && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "Env:$($Matches[1])" -Value $Matches[2] }
}
if ($args.Count -gt 0) {
    & $args[0] @($args | Select-Object -Skip 1)
    exit $LASTEXITCODE
}