@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\build-rust.ps1"
if errorlevel 1 exit /b %errorlevel%
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\build-host.ps1"
if errorlevel 1 exit /b %errorlevel%
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\build-rage-cli.ps1"
