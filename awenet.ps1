Set-Location $PSScriptRoot

$exe = Join-Path $PSScriptRoot "target\release\awe-node.exe"

if (-not (Test-Path $exe)) {
    Write-Host "[AWEp2P] First run: building the product..." -ForegroundColor Cyan
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Host ""
        Write-Host "[AWEp2P] Rust/Cargo is not installed." -ForegroundColor Red
        Write-Host "Install Rust from https://rustup.rs/ and run awenet.ps1 again."
        exit 1
    }

    cargo build --release -p awe-node
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[AWEp2P] Build failed." -ForegroundColor Red
        exit $LASTEXITCODE
    }
}

Write-Host "[AWEp2P] Starting..." -ForegroundColor Green
& $exe @args
exit $LASTEXITCODE
