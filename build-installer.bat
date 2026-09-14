@echo off
rem DeployCode 一键打包脚本：在沙箱外运行，绕过 TRAE 沙箱对系统缓存目录的限制
chcp 65001 >nul
echo ================================================
echo  DeployCode 打包（生成 exe 与 NSIS 安装包）
echo ================================================

rem 将 cargo 加入 PATH
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

where cargo >nul 2>nul
if errorlevel 1 (
    echo [错误] 未找到 cargo，请确认 Rust 已安装
    pause
    exit /b 1
)

cd /d "%~dp0"
echo.
echo [1/2] 开始打包，首次运行需要下载 NSIS 工具包，请耐心等待...
npm run app:build
if errorlevel 1 (
    echo.
    echo [错误] 打包失败，请查看上方日志
    pause
    exit /b 1
)

echo.
echo [2/2] 打包完成！产物位置：
echo   主程序:  target\release\deploy-code.exe
echo   安装包:  target\release\bundle\nsis\*.exe
echo.
explorer "%~dp0target\release\bundle\nsis"
pause
