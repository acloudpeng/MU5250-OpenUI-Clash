@echo off
rem Updates the modem to the OpenUI release that matches this checkout: scripts\windows\deploy.ps1.
setlocal
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\windows\deploy.ps1" %*
set status=%errorlevel%
echo.
pause
exit /b %status%
