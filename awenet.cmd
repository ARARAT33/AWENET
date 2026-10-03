@echo off
setlocal
cd /d "%~dp0"

set "EXE=%~dp0bin\windows-x86_64\awe-node.exe"
set "ZIP=%~dp0AWEP2P-Windows-x64-Portable.zip"
set "URL=https://github.com/ARARAT33/AWEP2P/releases/download/latest/AWEP2P-Windows-x64-Portable.zip"

if not exist "%EXE%" (
  echo [AWEp2P] Downloading latest ready-to-run Windows package...
  if not exist "%~dp0bin\windows-x86_64" mkdir "%~dp0bin\windows-x86_64"
  powershell -NoProfile -ExecutionPolicy Bypass -Command "try { Invoke-WebRequest -Uri '%URL%' -OutFile '%ZIP%' -UseBasicParsing -ErrorAction Stop; Expand-Archive -Path '%ZIP%' -DestinationPath '%~dp0' -Force; Remove-Item '%ZIP%' -Force -ErrorAction SilentlyContinue } catch { exit 1 }"
  if errorlevel 1 (
    del /q "%ZIP%" >nul 2>nul
    echo [AWEp2P] Latest Windows package could not be downloaded.
    exit /b 1
  )
)
if not exist "%EXE%" (
  echo [AWEp2P] awe-node.exe is missing from the downloaded package.
  exit /b 1
)
echo [AWEp2P] Starting...
"%EXE%" %*
endlocal
