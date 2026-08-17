@echo off
setlocal

pushd "%~dp0.." || exit /b 1

call pnpm tauri build --no-bundle
if errorlevel 1 (
    echo.
    echo k-Coder build failed. Deployment was skipped.
    popd
    exit /b 1
)

powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0deploy.ps1"
set "DEPLOY_EXIT_CODE=%ERRORLEVEL%"

popd
exit /b %DEPLOY_EXIT_CODE%
