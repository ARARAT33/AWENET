@echo off
setlocal
cd /d "%~dp0"

if not exist "target\release\awe-node.exe" (
  echo [AWEp2P] First run: building the product...
  where cargo >nul 2>nul
  if errorlevel 1 (
    echo.
    echo [AWEp2P] Rust/Cargo is not installed.
    echo Install Rust from https://rustup.rs/ and run awenet again.
    exit /b 1
  )
  cargo build --release -p awe-node
  if errorlevel 1 (
    echo.
    echo [AWEp2P] Build failed.
    exit /b 1
  )
)

echo [AWEp2P] Starting...
target\release\awe-node.exe %*
endlocal
