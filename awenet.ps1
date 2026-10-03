Set-Location $PSScriptRoot

$exe = Join-Path $PSScriptRoot "bin\windows-x86_64\awe-node.exe"
$zip = Join-Path $PSScriptRoot "AWEP2P-Windows-x64-Portable.zip"
$url = "https://github.com/ARARAT33/AWEP2P/releases/download/latest/AWEP2P-Windows-x64-Portable.zip"

if (-not (Test-Path $exe)) {
    Write-Host "[AWEp2P] Downloading latest ready-to-run Windows package..." -ForegroundColor Cyan
    try {
        Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing -ErrorAction Stop
        Expand-Archive -Path $zip -DestinationPath $PSScriptRoot -Force
        Remove-Item $zip -Force -ErrorAction SilentlyContinue
    } catch {
        Remove-Item $zip -Force -ErrorAction SilentlyContinue
        Write-Host "[AWEp2P] Latest Windows package could not be downloaded." -ForegroundColor Red
        Write-Host $_.Exception.Message -ForegroundColor Red
        exit 1
    }
}
if (-not (Test-Path $exe)) { Write-Host "[AWEp2P] awe-node.exe is missing from the downloaded package." -ForegroundColor Red; exit 1 }
Write-Host "[AWEp2P] Starting..." -ForegroundColor Green
& $exe @args
exit $LASTEXITCODE
