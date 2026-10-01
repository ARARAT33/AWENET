@echo off
setlocal
cd /d "%~dp0"

set "EXE=%~dp0bin\windows-x86_64\awe-node.exe"
set "URL=https://raw.githubusercontent.com/ARARAT33/AWEP2P/main/bin/windows-x86_64/awe-node.exe"

if not exist "%EXE%" (
  echo [AWEp2P] Downloading ready-to-run Windows product...
  if not exist "%~dp0bin\windows-x86_64" mkdir "%~dp0bin\windows-x86_64"
  powershell -NoProfile -ExecutionPolicy Bypass -Command "try { Invoke-WebRequest -Uri '%URL%' -OutFile '%EXE%' -UseBasicParsing -ErrorAction Stop } catch { exit 1 }"
  if errorlevel 1 (
    del /q "%EXE%" >nul 2>nul
    echo [AWEp2P] Ready binary is not published yet.
    echo [AWEp2P] Please run awenet again after GitHub Actions finishes.
    exit /b 1
  )
)

echo [AWEp2P] Starting...
"%EXE%" %*
endlocal
