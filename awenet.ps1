Set-Location $PSScriptRoot

$exe = Join-Path $PSScriptRoot "bin\windows-x86_64\awe-node.exe"
$url = "https://raw.githubusercontent.com/ARARAT33/AWEP2P/main/bin/windows-x86_64/awe-node.exe"

if (-not (Test-Path $exe)) {
    Write-Host "[AWEp2P] Downloading ready-to-run Windows product..." -ForegroundColor Cyan
    New-Item -ItemType Directory -Force -Path (Split-Path $exe) | Out-Null
    try {
        Invoke-WebRequest -Uri $url -OutFile $exe -UseBasicParsing
    } catch {
        Remove-Item $exe -Force -ErrorAction SilentlyContinue
        Write-Host "[AWEp2P] Ready binary is not published yet." -ForegroundColor Red
        Write-Host "[AWEp2P] Please run this command again after GitHub Actions finishes."
        exit 1
    }
}

Write-Host "[AWEp2P] Starting..." -ForegroundColor Green
& $exe @args
exit $LASTEXITCODE
